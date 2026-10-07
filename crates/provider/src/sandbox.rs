//! One Docker container per lease, driven through the `docker` CLI.
//!
//! Containers carry their lease id and SSH port as labels, so Docker itself is the record of
//! what runs and the provider can restart at any time. The provider also starts them (they do
//! not restart on their own), so a home is always mounted before its sandbox runs, including
//! after a reboot. Tenants share one machine, so each sandbox is fenced in on every axis:
//!
//! - **identity**: its own user id, so even a process escaping its container is a stranger
//!   to every other tenant's files;
//! - **filesystem**: a read-only root, an in-memory `/tmp`, and a home on a volume of its own
//!   with a fixed size, so it cannot fill the disk for others;
//! - **privileges**: every capability dropped and no way to gain any;
//! - **memory**: a guaranteed floor, a hard cap, and first in line for the OOM killer when it
//!   uses more than its share, so a greedy sandbox only hurts itself;
//! - **CPU and processes**: equal CPU shares, a core cap and a process limit;
//! - **network**: the internet only; firewall rules keep sandboxes away from each other, from
//!   the host and from private networks, so nothing local (such as the TEE's guest agent) is
//!   reachable from inside.
//!
//! Wiping a lease removes the container, its volume and its files, so nothing outlives it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context};
use protocol::api::{Connection, Lease, OfferSpec};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::config::SandboxConfig;

/// The sandbox image's build context, compiled in so the provider is self-contained.
const IMAGE_FILES: [(&str, &str); 5] = [
    ("Dockerfile", include_str!("../sandbox/Dockerfile")),
    ("start", include_str!("../sandbox/start")),
    ("agent", include_str!("../sandbox/agent")),
    ("templates/agent.py", include_str!("../sandbox/templates/agent.py")),
    ("templates/TASK.md", include_str!("../sandbox/templates/TASK.md")),
];
const LEASE_LABEL: &str = "agentcloud.lease";
const PORT_LABEL: &str = "agentcloud.port";
const NETWORK: &str = "agentcloud";
/// The Linux bridge behind [`NETWORK`], named so firewall rules can match it.
const BRIDGE: &str = "agentcloud0";
/// Ranges no sandbox may open connections to: private, carrier-grade NAT and link-local.
const PRIVATE_RANGES: [&str; 5] = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "169.254.0.0/16"];
/// The account renters log in as and the port its sshd listens on, both set by the Dockerfile.
const USER: &str = "agent";
const SSH_PORT: u16 = 2222;
/// Sandbox `n` (by port) runs as user and group `FIRST_UID + n`.
const FIRST_UID: u32 = 10_000;

/// A sandbox found on this host.
pub struct Sandbox {
    pub lease_id: String,
    pub port: u16,
    pub running: bool,
}

/// What goes into a new sandbox besides the renter's key.
pub struct Provision<'a> {
    pub lease: &'a Lease,
    pub wallet_mnemonic: &'a str,
    /// A JSON description of the lease for the agent: id, market, rent and where it is paid.
    pub lease_json: &'a str,
}

pub struct Sandboxes {
    config: SandboxConfig,
    /// A prebuilt image pinned by digest, or one built here and tagged with a hash of its
    /// files, so an updated provider gets a fresh image while running sandboxes keep theirs.
    image: String,
    /// The image's `/etc/passwd`, rewritten per sandbox to give `agent` its own user id.
    passwd: String,
    group: String,
    /// Per-sandbox files (passwd, group, disk image), as this process sees them...
    files: PathBuf,
    /// ...and as the Docker daemon sees them, which differs when the provider itself runs in
    /// a container.
    host_files: PathBuf,
}

impl Sandboxes {
    /// Pulls or builds the image and creates the network if they do not exist yet.
    pub async fn prepare(config: SandboxConfig, data_dir: &Path) -> anyhow::Result<Self> {
        let image = match &config.image {
            Some(image) => image.clone(),
            None => {
                let digest = IMAGE_FILES
                    .iter()
                    .fold(Sha256::new(), |hash, (path, text)| hash.chain_update(path).chain_update(text));
                format!("agentcloud-sandbox:{}", &hex::encode(digest.finalize())[..12])
            }
        };
        if docker(&["image", "inspect", &image]).await.is_err() {
            if config.image.is_some() {
                tracing::info!("pulling sandbox image {image}");
                docker(&["pull", &image]).await?;
            } else {
                tracing::info!("building sandbox image {image}, this takes a few minutes the first time");
                build_image(&image, &data_dir.join("sandbox-image")).await?;
            }
        }
        let bridge_format = "{{index .Options \"com.docker.network.bridge.name\"}}";
        match docker(&["network", "inspect", "--format", bridge_format, NETWORK]).await {
            Ok(bridge) if bridge.trim() == BRIDGE => {}
            Ok(bridge) => bail!("network {NETWORK} runs on bridge '{}', not {BRIDGE}; remove it", bridge.trim()),
            Err(_) => {
                let icc = "com.docker.network.bridge.enable_icc=false";
                let name = format!("com.docker.network.bridge.name={BRIDGE}");
                docker(&["network", "create", "-o", icc, "-o", &name, NETWORK]).await?;
            }
        }
        let passwd = docker(&["run", "--rm", "--entrypoint", "cat", &image, "/etc/passwd"]).await?;
        let group = docker(&["run", "--rm", "--entrypoint", "cat", &image, "/etc/group"]).await?;
        let files = std::path::absolute(data_dir.join("sandboxes"))?;
        std::fs::create_dir_all(&files)?;
        let host_files = host_path(&std::path::absolute(data_dir)?).await?.join("sandboxes");
        let sandboxes = Self { config, image, passwd, group, files, host_files };
        if sandboxes.config.firewall {
            sandboxes.install_firewall().await.context("installing the sandbox firewall")?;
        }
        Ok(sandboxes)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<Sandbox>> {
        let format = format!("{{{{.Label \"{LEASE_LABEL}\"}}}}\t{{{{.Label \"{PORT_LABEL}\"}}}}\t{{{{.State}}}}");
        let output = docker(&["ps", "--all", "--filter", &format!("label={LEASE_LABEL}"), "--format", &format]).await?;
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut fields = line.split('\t');
                let (Some(lease_id), Some(port), Some(state)) = (fields.next(), fields.next(), fields.next()) else {
                    bail!("unexpected docker ps output: {line}");
                };
                let port = port.parse().context("bad port label")?;
                Ok(Sandbox { lease_id: lease_id.to_string(), port, running: state == "running" })
            })
            .collect()
    }

    /// Starts a sandbox on the first port no other sandbox uses.
    pub async fn create(&self, provision: Provision<'_>, offer: &OfferSpec, taken: &[u16]) -> anyhow::Result<Sandbox> {
        let lease = provision.lease;
        let port = (self.config.first_port..=self.config.last_port)
            .find(|port| !taken.contains(port))
            .context("no free sandbox port")?;
        let uid = FIRST_UID + u32::from(port - self.config.first_port);
        self.write_identity(&lease.id, uid)?;
        self.mount_home(&lease.id, uid, offer.disk_gb).await?;

        let host_dir = self.host_files.join(&lease.id);
        let config = &self.config;
        let args = [
            "run".to_string(),
            "--detach".into(),
            format!("--name={}", container_name(&lease.id)),
            format!("--hostname=lease-{}", lease.id),
            format!("--label={LEASE_LABEL}={}", lease.id),
            format!("--label={PORT_LABEL}={port}"),
            format!("--runtime={}", config.runtime),
            format!("--user={uid}:{uid}"),
            format!("--volume={}:/etc/passwd:ro", host_dir.join("passwd").display()),
            format!("--volume={}:/etc/group:ro", host_dir.join("group").display()),
            format!("--volume={}:/home/{USER}", host_dir.join("home").display()),
            "--read-only".into(),
            format!("--tmpfs=/tmp:size={}m,mode=1777", config.tmp_mb),
            "--cap-drop=ALL".into(),
            "--security-opt=no-new-privileges".into(),
            format!("--cpus={}", offer.cpus),
            format!("--memory={}m", offer.memory_mb),
            format!("--memory-swap={}m", offer.memory_mb),
            format!("--memory-reservation={}m", config.memory_reserved_mb),
            "--oom-score-adj=500".into(),
            format!("--pids-limit={}", config.pids_limit),
            format!("--network={NETWORK}"),
            format!("--publish={port}:{SSH_PORT}"),
            "--restart=no".into(),
            format!("--env=SSH_PUBLIC_KEY={}", lease.ssh_key),
            format!("--env=AGENT_WALLET_MNEMONIC={}", provision.wallet_mnemonic.trim()),
            format!("--env=AGENT_LEASE_JSON={}", provision.lease_json),
            self.image.clone(),
        ];
        docker(&args.iter().map(String::as_str).collect::<Vec<_>>()).await?;
        Ok(Sandbox { lease_id: lease.id.clone(), port, running: true })
    }

    /// Starts a stopped sandbox again, after a reboot or a crash, remounting its home first.
    pub async fn start(&self, sandbox: &Sandbox, offer: &OfferSpec) -> anyhow::Result<()> {
        let uid = FIRST_UID + u32::from(sandbox.port - self.config.first_port);
        self.mount_home(&sandbox.lease_id, uid, offer.disk_gb).await?;
        docker(&["start", &container_name(&sandbox.lease_id)]).await.map(drop)
    }

    /// How renters reach the sandbox on host port `port`.
    pub fn connection(&self, offer: &OfferSpec, port: u16) -> Connection {
        let host = match self.config.connect_host.as_str() {
            "" => offer.host.clone(),
            template => template.replace("{port}", &port.to_string()),
        };
        let port = if self.config.connect_port == 0 { port } else { self.config.connect_port };
        Connection { host, port, user: USER.into(), tls: self.config.connect_tls }
    }

    /// Stops the sandbox and deletes everything in it.
    pub async fn wipe(&self, lease_id: &str) -> anyhow::Result<()> {
        docker(&["rm", "--force", &container_name(lease_id)]).await?;
        let home = self.host_files.join(lease_id).join("home");
        self.as_host_root(&format!("umount -l '{}' 2>/dev/null || true", home.display())).await?;
        let dir = self.files.join(lease_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        Ok(())
    }

    /// The sandbox's `/etc/passwd` and `/etc/group`, with `agent` as user and group `uid`.
    fn write_identity(&self, lease_id: &str, uid: u32) -> anyhow::Result<()> {
        let dir = self.files.join(lease_id);
        std::fs::create_dir_all(&dir)?;
        let with_uid = |file: &str, line: String| {
            let mut lines: Vec<String> =
                file.lines().filter(|l| !l.starts_with(&format!("{USER}:"))).map(String::from).collect();
            lines.push(line);
            lines.join("\n") + "\n"
        };
        std::fs::write(
            dir.join("passwd"),
            with_uid(&self.passwd, format!("{USER}:x:{uid}:{uid}::/home/{USER}:/bin/bash")),
        )?;
        std::fs::write(dir.join("group"), with_uid(&self.group, format!("{USER}:x:{uid}:")))?;
        Ok(())
    }

    /// Mounts the lease's home, owned by `uid`. With a disk quota it is an ext4 image of
    /// `disk_gb`, created once and loop-mounted, so a full home stops at its own limit. Where
    /// loop mounts are impossible it falls back to a plain directory and says so.
    async fn mount_home(&self, lease_id: &str, uid: u32, disk_gb: u32) -> anyhow::Result<()> {
        let home = self.files.join(lease_id).join("home");
        std::fs::create_dir_all(&home)?;
        if self.config.disk_quota {
            match self.mount_quota_home(lease_id, uid, disk_gb).await {
                Ok(()) => return Ok(()),
                Err(error) => tracing::warn!(lease = lease_id, "disk quota unavailable, home is unbounded: {error:#}"),
            }
        }
        std::os::unix::fs::chown(&home, Some(uid), Some(uid)).context("handing the home to its user")
    }

    async fn mount_quota_home(&self, lease_id: &str, uid: u32, disk_gb: u32) -> anyhow::Result<()> {
        let image = self.files.join(lease_id).join("home.img");
        if !image.exists() {
            // Sparse: only what the renter writes takes space on the real disk.
            std::fs::File::create(&image)?.set_len(u64::from(disk_gb) << 30)?;
            let output = Command::new("mkfs.ext4").args(["-q", "-F", "-m", "0"]).arg(&image).output().await?;
            if !output.status.success() {
                bail!("mkfs.ext4 failed: {}", String::from_utf8_lossy(&output.stderr).trim());
            }
        }
        let dir = self.host_files.join(lease_id);
        let (image, home) = (dir.join("home.img"), dir.join("home"));
        // Verified afterwards: a mount that silently did not happen would leave the home
        // unbounded on the shared disk.
        let script = format!(
            "set -e
             mountpoint -q '{home}' || mount -o loop '{image}' '{home}'
             mountpoint -q '{home}'
             chown {uid}:{uid} '{home}'",
            home = home.display(),
            image = image.display()
        );
        self.as_host_root(&script).await.map(drop)
    }

    /// Firewall rules on the host: a sandbox may not open connections to the host itself or to
    /// private networks; replies to connections made to it still flow. Idempotent.
    async fn install_firewall(&self) -> anyhow::Result<()> {
        let private = PRIVATE_RANGES
            .iter()
            .map(|range| format!("add DOCKER-USER -i {BRIDGE} -d {range} -j DROP"))
            .collect::<Vec<_>>();
        let script = format!(
            "set -e
             # Use the iptables backend Docker itself uses.
             for ipt in iptables-nft iptables-legacy; do $ipt -n -L DOCKER-USER >/dev/null 2>&1 && break; done
             add() {{ $ipt -C \"$@\" 2>/dev/null || $ipt -I \"$@\"; }}
             add INPUT -i {BRIDGE} -j DROP
             add INPUT -i {BRIDGE} -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
             {private}
             # DNS to the host's own resolvers, which may sit in a private range.
             for ns in $(awk '/^nameserver/ && $2 !~ /:/ {{print $2}}' /etc/resolv.conf); do
               for proto in udp tcp; do
                 add DOCKER-USER -i {BRIDGE} -d $ns -p $proto --dport 53 -j RETURN
                 add INPUT -i {BRIDGE} -d $ns -p $proto --dport 53 -j ACCEPT
               done
             done
             add DOCKER-USER -i {BRIDGE} -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN",
            private = private.join("\n             ")
        );
        docker(&[
            "run",
            "--rm",
            "--privileged",
            "--user=0",
            "--network=host",
            "--entrypoint",
            "sh",
            &self.image,
            "-c",
            &script,
        ])
        .await?;
        tracing::info!("sandbox firewall in place on {BRIDGE}");
        Ok(())
    }

    /// Runs `script` as root in the Docker daemon's mount namespace, from a short-lived
    /// privileged container, so mounts land where the daemon (and so every sandbox) sees them,
    /// whatever the host's mount propagation. Paths are as the daemon reports them. The
    /// provider holds the Docker socket, so this grants nothing it does not already have.
    async fn as_host_root(&self, script: &str) -> anyhow::Result<String> {
        let enter = format!(
            "for p in /proc/[0-9]*; do [ \"$(cat $p/comm 2>/dev/null)\" = dockerd ] && d=${{p#/proc/}} && break; done
             [ -n \"$d\" ] || {{ echo 'dockerd not found' >&2; exit 1; }}
             exec nsenter -t \"$d\" -m -- sh -c {}",
            shell_quote(script)
        );
        let args = ["run", "--rm", "--privileged", "--user=0", "--pid=host", "--network=none", "--entrypoint", "sh"];
        docker(&args.iter().copied().chain([self.image.as_str(), "-c", &enter]).collect::<Vec<_>>()).await
    }
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn container_name(lease_id: &str) -> String {
    format!("agentcloud-{lease_id}")
}

/// Where `path` lives on the Docker host. Outside a container that is `path` itself; inside
/// one, it is the source of the mount that holds it.
async fn host_path(path: &Path) -> anyhow::Result<PathBuf> {
    if !Path::new("/.dockerenv").exists() {
        return Ok(path.to_path_buf());
    }
    let me = std::fs::read_to_string("/etc/hostname")?.trim().to_string();
    let format = "{{range .Mounts}}{{.Destination}}\t{{.Source}}\n{{end}}";
    let mounts = docker(&["inspect", "--format", format, &me]).await?;
    mounts
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(destination, _)| path.starts_with(destination))
        .max_by_key(|(destination, _)| destination.len())
        .map(|(destination, source)| Path::new(source).join(path.strip_prefix(destination).unwrap_or(path)))
        .with_context(|| format!("{} is not on a mounted volume", path.display()))
}

/// Writes the build context to `dir` and builds it. The context goes over the Docker API, so
/// `dir` need not be visible to the daemon.
async fn build_image(tag: &str, dir: &Path) -> anyhow::Result<()> {
    for (path, text) in IMAGE_FILES {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().expect("files sit in a directory"))?;
        std::fs::write(&file, text)?;
    }
    let output = Command::new("docker")
        .args(["build", "--quiet", "--tag", tag])
        .arg(dir)
        .stdin(Stdio::null())
        .output()
        .await
        .context("running docker")?;
    if !output.status.success() {
        bail!("docker build failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

/// Runs `docker` and returns its stdout, failing with its stderr.
async fn docker(args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("docker").args(args).output().await.context("running docker")?;
    if !output.status.success() {
        bail!("docker {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
