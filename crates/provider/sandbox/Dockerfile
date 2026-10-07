# The image every lease runs in: Ubuntu with the common agent toolchains, an SSH server that
# accepts only the renter's key, and the `agent` command that runs and manages the agent.
#
# Everything runs as an unprivileged user with every capability dropped, on a read-only root
# filesystem. The home directory is the lease's own volume with a fixed size, so it is where
# everything persistent goes: the agent in ~/app, secrets in ~/.env, pip --user, npm globals.
# Written without BuildKit features so it builds on any Docker.
FROM ubuntu:24.04

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      openssh-server ca-certificates curl wget git gnupg python3 python3-pip python3-venv \
      jq less nano vim-tiny tmux unzip iptables \
 && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/* /etc/ssh/ssh_host_*

# The agent CLIs and SDKs most agents are built on, preinstalled so they cost no disk quota.
RUN npm install -g --omit=dev @anthropic-ai/claude-code @openai/codex @google/gemini-cli \
 && npm cache clean --force \
 && pip install --break-system-packages --no-cache-dir \
      anthropic openai google-genai mistralai requests httpx python-dotenv

# Users install into their home: pip --user and npm globals under ~/.local.
RUN printf '[global]\nbreak-system-packages = true\n' > /etc/pip.conf \
 && printf 'prefix=${HOME}/.local\n' > /etc/npmrc \
 && printf '%s\n' \
      'export PATH="$HOME/.local/bin:$PATH"' \
      'export DISABLE_AUTOUPDATER=1' \
      '[ -f "$HOME/.env" ] && set -a && . "$HOME/.env" && set +a' \
      > /etc/profile.d/agentcloud.sh

RUN userdel -r ubuntu 2>/dev/null; useradd -m -u 1000 -s /bin/bash agent \
 && mkdir -p /run/sshd \
 && printf '%s\n' \
      'Port 2222' \
      'HostKey /home/agent/.ssh/host_ed25519_key' \
      'PidFile none' \
      'UsePAM no' \
      'PasswordAuthentication no' \
      'KbdInteractiveAuthentication no' \
      'AllowUsers agent' \
      'AuthorizedKeysFile .ssh/authorized_keys' \
      'Subsystem sftp internal-sftp' \
      > /etc/ssh/agentcloud_sshd_config

COPY start agent /usr/local/bin/
COPY templates /usr/local/share/agentcloud/templates
RUN chmod 755 /usr/local/bin/start /usr/local/bin/agent

USER agent
WORKDIR /home/agent
ENV HOME=/home/agent
EXPOSE 2222
CMD ["/usr/local/bin/start"]
