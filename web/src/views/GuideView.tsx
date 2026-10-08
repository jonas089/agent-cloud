// Getting started: from nothing to a running agent in a few commands, what the sandbox
// offers, and in plain words why nobody else can read your API keys.

import { market } from "../api";
import type { Session } from "../App";
import { shorten, tia } from "../format";
import { contractUrl } from "../governance";
import { CodeBlock, usePolling } from "../ui";

const SECTIONS = [
  { id: "quickstart", title: "Quickstart" },
  { id: "agents", title: "Agent types" },
  { id: "own", title: "Deploy your own code" },
  { id: "manage", title: "Manage it" },
  { id: "safe", title: "Why your keys are safe" },
  { id: "code", title: "Who controls the code" },
  { id: "limits", title: "Resources" },
  { id: "pay", title: "Paying" },
];

export function GuideView({ session }: { session: Session }) {
  const { config } = session;
  const offers = usePolling(market.offers, 60_000);
  const governed = (offers.value ?? []).filter((offer) => offer.governance);

  return (
    <div className="docs">
      <aside className="docs-toc">
        <nav>
          {SECTIONS.map((s) => (
            <a
              key={s.id}
              href="#guide"
              onClick={(e) => {
                e.preventDefault();
                document.getElementById(s.id)?.scrollIntoView({ behavior: "smooth" });
              }}
            >
              {s.title}
            </a>
          ))}
        </nav>
      </aside>
      <article className="docs-body">
        <h1>Getting started</h1>
        <p className="lead">
          A private Linux sandbox for your AI agent, inside a hardware enclave. Claude Code, Codex, Gemini CLI and the
          OpenAI, Anthropic and Google SDKs are already installed. Your agent gets its own wallet and pays its own rent.
        </p>

        <section id="quickstart">
          <h2>Quickstart</h2>
          <ol className="docs-steps">
            <li>
              <strong>Rent.</strong> Connect <a href="https://www.keplr.app" target="_blank" rel="noreferrer">Keplr</a>,
              press Rent on the Market tab, let the page make you an SSH key, and approve the first payment.
            </li>
            <li>
              <strong>Fund.</strong> On Manage agents, send your agent's wallet a few TIA. It pays its rent from there,
              every hour.
            </li>
            <li>
              <strong>Connect.</strong> Copy the SSH block from Manage agents into <code>~/.ssh/config</code>, then:
              <CodeBlock code="ssh agentcloud-<lease>" />
            </li>
            <li>
              <strong>Start an agent.</strong> Pick a model, paste your API key when asked, and say what it should do:
              <CodeBlock code={"agent init openai\nagent secret set OPENAI_API_KEY\nnano ~/app/TASK.md\nagent logs -f"} />
            </li>
          </ol>
          <p>
            That's it. The agent runs every ten minutes, keeps notes between runs, and restarts itself if it crashes.
            Name the exact API or URL it should use in <code>TASK.md</code>: models guess endpoints from memory, and
            those guesses go stale.
          </p>
        </section>

        <section id="agents">
          <h2>Agent types</h2>
          <table className="docs-table">
            <tbody>
              <tr>
                <td>agent init openai</td>
                <td>OPENAI_API_KEY</td>
                <td>GPT with a shell tool. Light and cheap; the default choice.</td>
              </tr>
              <tr>
                <td>agent init claude</td>
                <td>ANTHROPIC_API_KEY</td>
                <td>Claude with a shell tool.</td>
              </tr>
              <tr>
                <td>agent init gemini</td>
                <td>GEMINI_API_KEY</td>
                <td>Gemini with a shell tool.</td>
              </tr>
              <tr>
                <td>agent init grok · deepseek · mistral · openrouter</td>
                <td>XAI_API_KEY, …</td>
                <td>The same agent on any OpenAI-compatible API. OpenRouter reaches hundreds of models with one key.</td>
              </tr>
              <tr>
                <td>agent init claude-code</td>
                <td>ANTHROPIC_API_KEY</td>
                <td>Claude Code, run headless on your task each round.</td>
              </tr>
              <tr>
                <td>agent init codex</td>
                <td>OPENAI_API_KEY</td>
                <td>OpenAI Codex, run headless on your task each round.</td>
              </tr>
              <tr>
                <td>agent init gemini-cli</td>
                <td>GEMINI_API_KEY</td>
                <td>Gemini CLI. Memory hungry; run little else beside it.</td>
              </tr>
            </tbody>
          </table>
          <p>
            Every type reads <code>~/app/TASK.md</code> and writes notes to <code>~/app/MEMORY.md</code>. Change the model
            with <code>agent secret set MODEL gpt-5.5</code> and how often it runs with{" "}
            <code>agent secret set AGENT_INTERVAL 300</code> (seconds).
          </p>
        </section>

        <section id="own">
          <h2>Deploy your own code</h2>
          <p>
            Anything with an <code>agent.sh</code> in its root runs: Python, Node, a trading bot, an MCP server. Push it
            from your laptop and it installs <code>requirements.txt</code> or <code>package.json</code> and restarts:
          </p>
          <CodeBlock code={"git remote add cloud agentcloud-<lease>:app.git\ngit push cloud main"} />
        </section>

        <section id="manage">
          <h2>Manage it</h2>
          <table className="docs-table">
            <tbody>
              <tr>
                <td>agent status</td>
                <td>health</td>
                <td>Running or not, restarts, memory, disk and which secrets are set.</td>
              </tr>
              <tr>
                <td>agent logs -f</td>
                <td>output</td>
                <td>Follow what the agent prints.</td>
              </tr>
              <tr>
                <td>agent secret set NAME</td>
                <td>keys</td>
                <td>
                  Store or replace a key; the agent restarts with it. <code>agent secret set openai</code> works too. Keys
                  are hidden from the agent's logs and from what it sends the model.
                </td>
              </tr>
              <tr>
                <td>agent restart · stop · start</td>
                <td>control</td>
                <td>Restart after edits, or pause it.</td>
              </tr>
              <tr>
                <td>agent once</td>
                <td>test</td>
                <td>Run one round now, in your terminal.</td>
              </tr>
              <tr>
                <td>agent wallet</td>
                <td>money</td>
                <td>The agent's address, balance, and until when its rent is paid.</td>
              </tr>
            </tbody>
          </table>
        </section>

        <section id="safe">
          <h2>Why your keys are safe</h2>
          <ol className="docs-steps">
            <li>
              <strong>A sealed machine.</strong> Your sandbox runs inside an Intel TDX enclave. The processor encrypts its
              memory and disk, so the data center, the cloud host and we only ever see scrambled bytes.
            </li>
            <li>
              <strong>Nobody holds a master key.</strong> The enclave runs a locked image without remote login, us
              included. The exact code inside is fixed and public, and the enclave proves it with a signed attestation you
              can check from any offer.
            </li>
            <li>
              <strong>Your keys never pass through us.</strong> You paste them straight into the enclave over SSH, which is
              encrypted from your laptop to the sandbox. The website and market never see them.
            </li>
            <li>
              <strong>Only your SSH key opens your sandbox.</strong> Your first payment, signed in Keplr, commits to it on
              chain, and the enclave refuses any other key, even one we hand it.
            </li>
            <li>
              <strong>Walls between neighbours.</strong> Each sandbox has its own user, its own disk, its own memory and
              process space, and no network path to the others. <code>~/.env</code> is readable only by you.
            </li>
          </ol>
          <p>
            Two things to keep in mind: the AI provider of course sees the key you use with it, and so does any code you run
            in your own sandbox. Give agents keys with spending limits, like an exchange key that can trade but not
            withdraw.
          </p>
        </section>

        <section id="code">
          <h2>Who controls the code</h2>
          <p>
            <strong>What runs.</strong> The machine's configuration pins two images by digest: the provider, which starts
            and wipes sandboxes and pays rent, and the sandbox you log in to. Phala measures that configuration into the
            machine's attestation.
          </p>
          <p>
            <strong>What enforces it.</strong> Phala's KMS gives the machine its keys only if the configuration's hash is
            on the allowlist of the machine's contract on Base. Without those keys it cannot read its disk or prove who it
            is.
          </p>
          <p>
            <strong>Why it cannot change.</strong> Only the contract's owner can add a hash or upgrade the contract. The
            owner was renounced to the zero address and upgrades were disabled, so nobody can, us included. Check it on
            the contract page: <code>owner</code> reads <code>0x000…000</code>.
            {governed.length > 0 && " Contracts: "}
            {governed.map((offer, i) => (
              <span key={offer.id}>
                {i > 0 && ", "}
                <a href={contractUrl(offer.governance!)} target="_blank" rel="noreferrer">
                  {shorten(offer.governance!.app_contract, 4)}
                </a>
              </span>
            ))}
            {governed.length > 0 && "."}
          </p>
          <p>
            <strong>What we can still change.</strong> Price, number of slots, payout address and market address. They are
            inputs to the frozen code, not part of it, and none of them reach your files or keys. We can also shut a machine
            down; your agent's wallet mnemonic is in <code>~/.agentcloud</code>, so its funds do not depend on the machine.
          </p>
          <p>
            <strong>What you still trust.</strong> Intel TDX, Phala's KMS (its own contract, run by Phala), and, until host
            keys are part of the attestation, that your first <code>ssh</code> connection reaches this machine.
          </p>
        </section>

        <section id="limits">
          <h2>Resources</h2>
          <p>
            Ten sandboxes share one enclave. Each gets up to one CPU core and 768 MB of memory, with 128 MB always kept for
            it, 3 GB of disk (go over it and the agent pauses until you make room), and a fair share of the
            CPU when everyone is busy. That is plenty for API-driven
            agents: trading bots, monitors, research loops, chat bots and Claude Code or Codex on everyday tasks. It is not
            meant for running models locally or for heavy builds.
          </p>
        </section>

        <section id="pay">
          <h2>Paying</h2>
          <p>
            Rent is prepaid by the hour from the agent's wallet, a few minutes before the hour runs out, plus{" "}
            {config.typical_fee_utia ? `about ${tia(config.typical_fee_utia)}` : "a fraction of a cent of"} gas per
            transfer, priced by the chain at the time. The first hour comes from Keplr with a{" "}
            {tia(config.escrow_fee_utia)} escrow fee. If the wallet runs dry, the sandbox survives the grace period shown on
            the offer, then it is deleted with everything in it. Cancel any time on Manage agents: the sandbox is wiped and
            the wallet's balance comes back to you.
          </p>
        </section>
      </article>
    </div>
  );
}
