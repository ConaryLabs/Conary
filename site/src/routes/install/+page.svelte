<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import TerminalFrame from '$lib/components/TerminalFrame.svelte';
	import { previewRelease } from '$lib/preview-release';
</script>

<PageMeta
	title="Conary tester release status — Conary"
	description={`Conary ${previewRelease.tag} is published and verified; the packaged tester loop remains paused until the ordinary-package corpus gate passes.`}
	path="/install/"
/>

<PageIntro
	eyebrow="Tester loop paused"
	title="The release is verified; the tester runbook remains paused."
	description={previewRelease.testerAuthorityReason}
/>

<section class="install-section">
	<div class="container install-grid">
		<aside class="safety-rail" aria-labelledby="safety-title">
			<p class="eyebrow">Before you start</p>
			<h2 id="safety-title">Current stop gate</h2>
			<ul>
				<li>Do not use {previewRelease.tag} as current tester authority.</li>
				<li>Use a VM, snapshot, or explicitly non-critical host.</li>
				<li>The packaged tester lane is x86_64.</li>
				<li>Match Fedora 44, Ubuntu 26.04 LTS, or Arch Linux.</li>
				<li>Stop if the checksum does not print <code>OK</code>.</li>
				<li>Ask before every command that can mutate the host.</li>
			</ul>
			<a href={previewRelease.matrixUrl} class="btn btn-secondary">
				Read the release matrix <span aria-hidden="true">↗</span>
			</a>
		</aside>

		<div class="install-content">
			<section class="authority-pause" aria-labelledby="authority-pause-title">
				<p class="eyebrow">No current pinned release</p>
				<h2 id="authority-pause-title">The package runbook is not open.</h2>
				<p>
					{previewRelease.testerAuthorityReason} The exact {previewRelease.tag}
					artifacts are the current immutable release, but release proof is not the
					ordinary-package corpus proof required before outreach. This page will reopen
					only after that separate gate closes and a tester pin is assigned.
				</p>
				<div class="button-row">
					<a href={previewRelease.matrixUrl} class="btn btn-primary">
						Read exact release evidence <span aria-hidden="true">↗</span>
					</a>
					<a href={previewRelease.releaseUrl} class="btn btn-secondary">
						Inspect {previewRelease.tag} artifact <span aria-hidden="true">↗</span>
					</a>
				</div>
			</section>

			<section class="install-step" id="signed-bootstrap">
				<div class="step-heading">
					<span class="step-number">00</span>
					<div>
						<h2>Use the signed bootstrap protocol when its carrying release is pinned</h2>
						<p>
							The installer endpoint is inspectable now. It fails closed until the latest
							immutable release contains the signed <code>conary-bootstrap-v1.manifest</code>
							and its detached signature. Preview is the default; installation requires the
							explicit <code>--apply --yes</code> pair.
						</p>
					</div>
				</div>

				<TerminalFrame title="download, inspect, and preview">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">curl --proto '=https' --tlsv1.2 -fLO https://conary.io/install-conary-preview.sh</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">less install-conary-preview.sh</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">bash ./install-conary-preview.sh</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">bash ./install-conary-preview.sh --apply --yes</span></span>
				</TerminalFrame>

				<div class="boundary-note step-note">
					<strong>Do not pipe the endpoint into a shell.</strong>
					The script verifies product-signed release authority before selecting a host
					artifact, verifies that artifact before invoking the native package manager,
					and leaves system initialization to the native release package.
				</div>
			</section>

			<details class="retained-runbook">
				<summary>Retained {previewRelease.tag} commands — paused, do not run</summary>
				<section class="install-step" id="preflight">
				<div class="step-heading">
					<span class="step-number">01</span>
					<div>
						<h2>Confirm the host and the stop conditions</h2>
						<p>
							The basic package loop uses a stock distribution kernel. Composefs, UEFI,
							and special boot support are only needed for generation work outside this runbook.
						</p>
					</div>
				</div>

				<TerminalFrame title="host preflight">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">cat /etc/os-release</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">uname -m</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">uname -r</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo -v</span></span>
				</TerminalFrame>

				<div class="boundary-note step-note">
					<strong>Continue only with a matching test host.</strong>
					Expect Fedora 44, Ubuntu 26.04 LTS, or Arch Linux; <code>x86_64</code>;
					a stock kernel; and working <code>sudo</code>.
				</div>
			</section>

			<section class="install-step" id="download-verify">
				<div class="step-heading">
					<span class="step-number">02</span>
					<div>
						<h2>Download one package and verify it</h2>
						<p>
							Create a clean directory, choose only the package for this host, and verify it
							against the release checksum before installation.
						</p>
					</div>
				</div>

				<TerminalFrame title="clean preview directory">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">mkdir -p "{previewRelease.workDirectory}"</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">cd "{previewRelease.workDirectory}"</span></span>
				</TerminalFrame>

				<div class="download-grid">
					{#each previewRelease.targets as target}
						<article class="download-lane">
							<div class="lane-heading">
								<span>{target.name}</span>
								<code>{target.asset}</code>
							</div>
							<TerminalFrame title={`${target.name} download`}>
								<span class="terminal-line"><span class="terminal-command">base="{previewRelease.downloadBaseUrl}"</span></span>
								<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">curl -fLO "$base/SHA256SUMS"</span></span>
								<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">curl -fLO "$base/{target.asset}"</span></span>
								<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sha256sum -c SHA256SUMS --ignore-missing</span></span>
							</TerminalFrame>
						</article>
					{/each}
				</div>

				<p class="verification-line">
					<span aria-hidden="true"></span>
					The downloaded package must print <strong>OK</strong>. Stop on any missing or failed checksum.
				</p>
			</section>

			<section class="install-step" id="native-install">
				<div class="step-heading">
					<span class="step-number">03</span>
					<div>
						<h2>Install with the native package manager</h2>
						<p>
							Choose the command that matches the verified package. This changes the host,
							so review the command and ask for approval before running it.
						</p>
					</div>
				</div>

				<div class="install-lanes">
					{#each previewRelease.targets as target}
						<div class="install-lane">
							<div>
								<strong>{target.name}</strong>
								<span class="mutation-label">live mutation</span>
							</div>
							<code>{target.installCommand}</code>
						</div>
					{/each}
				</div>
			</section>

			<section class="install-step" id="confirm-setup">
				<div class="step-heading">
					<span class="step-number">04</span>
					<div>
						<h2>Confirm the installed version and repository</h2>
						<p>
							Release packages initialize one source-independent database and configure
							Remi plus all built-in RPM, Debian, and Arch source feeds. Inspect the
							configured sources before the package loop.
						</p>
					</div>
				</div>

				<TerminalFrame title="installed preview check">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">conary --version</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">conary --help | sed -n '1,80p'</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary repo list</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary repo sync remi</span></span>
				</TerminalFrame>

				<div class="boundary-note step-note">
					<strong>Repository sync changes local state.</strong>
					Ask before running <code>repo sync remi</code>. Packaged testers should not initialize
					the system again or add a second Remi repository.
				</div>
			</section>

			<section class="install-step" id="tester-loop">
				<div class="step-heading">
					<span class="step-number">05</span>
					<div>
						<h2>Run the bounded tester loop in order</h2>
						<p>
							Choose a source that differs from the host's native package format. Review
							every dry-run first and ask before each command with <code>--yes</code>.
						</p>
					</div>
				</div>

				<div class="loop-legend" aria-label="Command intent legend">
					<span><i class="intent-mark dry" aria-hidden="true"></i>inspect first</span>
					<span><i class="intent-mark live" aria-hidden="true"></i>live mutation</span>
					<span><i class="intent-mark read" aria-hidden="true"></i>read-only check</span>
				</div>

				<TerminalFrame title="first cross-distro tester loop">
					<span class="terminal-line intent-read"><span class="intent-label">choose</span><span class="terminal-command">source=ubuntu-26.04  # Fedora/Arch; use fedora-44 on Ubuntu</span></span>
						<span class="terminal-line intent-dry"><span class="intent-label">inspect</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --dry-run</span></span>
						<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --yes</span></span>
					<span class="terminal-line intent-read"><span class="intent-label">read</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary list htop --info</span></span>
					<span class="terminal-line intent-read"><span class="intent-label">read</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary query depends htop</span></span>
					<span class="terminal-line intent-dry"><span class="intent-label">inspect</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary update htop --dry-run</span></span>
					<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary remove htop --yes</span></span>
				</TerminalFrame>

				<div class="boundary-note step-note">
						<strong>Capability preflight and enforcement are automatic.</strong>
						Conary validates <code>htop</code>'s exact package declaration against the target
						before mutation and rejects unsupported requirements without a bypass flag.
				</div>
			</section>

				<section class="install-step report-step" id="report-feedback">
				<div class="step-heading">
					<span class="step-number">06</span>
					<div>
						<h2>Report the complete, partial, slow, or failed path</h2>
						<p>
							An exact capability error, lifecycle defect, or slow first conversion is
							useful evidence. Keep the public report concise and include only the output
							needed to explain the result.
						</p>
					</div>
				</div>

				<div class="report-grid">
					<div>
						<h3>Record</h3>
						<ul>
							<li>Exact commands and exit statuses.</li>
							<li>Distribution, kernel, architecture, and Conary version.</li>
							<li>Source package format and the host's native package format.</li>
							<li>Release tag, exact package filename, and checksum result.</li>
							<li>Whether the loop completed and where a partial run stopped.</li>
							<li>Anything confusing, slow, scary, or unexpectedly good.</li>
						</ul>
					</div>
					<div>
						<h3>Keep private</h3>
						<ul>
							<li>Complete transcripts and full package inventories.</li>
							<li>Broad environment dumps, raw logs, and live databases.</li>
							<li>Credentials, private keys, SSH keys, and shell history.</li>
							<li><code>/etc/conary/trust</code> and unreviewed support bundles.</li>
						</ul>
					</div>
				</div>

				<div class="button-row report-actions">
					<a href={previewRelease.feedbackUrl} class="btn btn-primary">
						Open pre-alpha feedback <span aria-hidden="true">↗</span>
					</a>
					<a href={previewRelease.testerGuideUrl} class="btn btn-secondary">
						Read the pinned guide <span aria-hidden="true">↗</span>
					</a>
				</div>
				</section>
			</details>

			<section class="contributor-panel" id="source-build">
				<div>
					<p class="eyebrow">Contributor path</p>
					<h2>Build a debug binary from source</h2>
					<p>
						Use this path for development, not as a substitute for the packaged tester lane.
						It requires Rust 1.97.1+, Git, and Linux; Conary does not currently build on macOS or Windows.
					</p>
				</div>

				<TerminalFrame title="developer build">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">git clone https://github.com/ConaryLabs/Conary.git</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">cd Conary</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">cargo build -p conary</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo install -m 755 target/debug/conary /usr/local/bin/</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system init</span></span>
				</TerminalFrame>

				<p class="profile-note">
					The example is for Fedora 44. Use <code>--profile ubuntu-26.04</code> or
					<code>--profile arch</code> only on the matching host.
				</p>
			</section>
		</div>
	</div>
</section>

<style>
	.install-section {
		padding: var(--section-space) 0 1rem;
	}

	.install-grid {
		display: grid;
		grid-template-columns: minmax(220px, 0.72fr) minmax(0, 2fr);
		gap: clamp(2.5rem, 7vw, 6rem);
		align-items: start;
	}

	.safety-rail {
		position: sticky;
		top: calc(var(--header-height) + 2rem);
		padding: 1.4rem;
		border: 1px solid var(--color-control-border);
		background: var(--color-layer);
	}

	.safety-rail h2 {
		margin-bottom: 1rem;
		font-size: var(--step-sub);
	}

	.safety-rail ul,
	.report-grid ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.safety-rail ul {
		margin-bottom: 1.4rem;
	}

	.safety-rail li,
	.report-grid li {
		position: relative;
		padding-left: 1.1rem;
		color: var(--color-mist);
	}

	.safety-rail li {
		padding-block: 0.72rem;
		border-bottom: 1px solid var(--color-border);
		font-size: 0.86rem;
	}

	.safety-rail li::before,
	.report-grid li::before {
		content: '';
		position: absolute;
		left: 0;
		width: 0.42rem;
		height: 0.42rem;
		background: var(--color-orange);
		transform: rotate(45deg);
	}

	.safety-rail li::before {
		top: 1.08rem;
	}

	.safety-rail .btn {
		width: 100%;
	}

	.install-content {
		min-width: 0;
	}

	.authority-pause {
		padding: clamp(1.5rem, 4vw, 2.5rem);
		margin-bottom: clamp(2.5rem, 6vw, 4rem);
		border: 1px solid var(--color-orange);
		background: var(--color-layer);
	}

	.authority-pause h2 {
		margin-bottom: 0.75rem;
		font-size: var(--step-section);
	}

	.authority-pause > p:not(.eyebrow) {
		max-width: 70ch;
		color: var(--color-mist);
	}

	.retained-runbook {
		margin-bottom: clamp(3rem, 7vw, 5rem);
		border-top: 1px solid var(--color-border);
	}

	.retained-runbook summary {
		padding: 1rem 0;
		color: var(--color-muted);
		cursor: pointer;
		font-family: var(--font-mono);
		font-size: 0.75rem;
		letter-spacing: 0.04em;
	}

	.install-step {
		scroll-margin-top: 7rem;
		padding-bottom: clamp(3.5rem, 8vw, 6rem);
		margin-bottom: clamp(3.5rem, 8vw, 6rem);
		border-bottom: 1px solid var(--color-border);
	}

	.step-heading {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		gap: 1rem;
		margin-bottom: 1.75rem;
	}

	.step-number {
		padding-top: 0.28rem;
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.72rem;
	}

	.step-heading h2,
	.contributor-panel h2 {
		margin-bottom: 0.65rem;
		font-size: var(--step-section);
	}

	.step-heading p,
	.contributor-panel > div > p:last-child,
	.profile-note {
		max-width: 68ch;
		margin: 0;
		color: var(--color-mist);
	}

	.step-note,
	.profile-note {
		margin-top: 1.25rem;
	}

	.download-grid {
		display: grid;
		gap: 1rem;
		margin-top: 1.25rem;
	}

	.download-lane {
		min-width: 0;
		padding: 1rem;
		border: 1px solid var(--color-border);
		background: rgb(16 29 48 / 45%);
	}

	.lane-heading {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 1rem;
		margin-bottom: 0.85rem;
	}

	.lane-heading span {
		font-weight: 600;
	}

	.lane-heading code {
		max-width: 65%;
		overflow-wrap: anywhere;
		text-align: right;
	}

	.verification-line {
		display: flex;
		gap: 0.75rem;
		align-items: flex-start;
		margin: 1.25rem 0 0;
		color: var(--color-mist);
	}

	.verification-line > span {
		width: 0.72rem;
		height: 0.72rem;
		margin-top: 0.45rem;
		flex: 0 0 auto;
		background: var(--color-cyan);
	}

	.verification-line strong {
		color: var(--color-ivory);
	}

	.install-lanes {
		border-top: 1px solid var(--color-border);
	}

	.install-lane {
		display: grid;
		grid-template-columns: minmax(180px, 0.72fr) minmax(0, 1.28fr);
		gap: 1rem;
		align-items: center;
		padding: 1.15rem 0;
		border-bottom: 1px solid var(--color-border);
	}

	.install-lane > div {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 0.25rem;
	}

	.install-lane code {
		overflow-wrap: anywhere;
		justify-self: end;
		text-align: right;
	}

	.mutation-label {
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.66rem;
		letter-spacing: 0.06em;
		text-transform: uppercase;
	}

	.loop-legend {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem 1.25rem;
		margin-bottom: 0.85rem;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: 0.68rem;
	}

	.loop-legend span {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
	}

	.intent-mark {
		width: 0.52rem;
		height: 0.52rem;
		flex: 0 0 auto;
	}

	.intent-mark.dry {
		border: 2px solid var(--color-cyan);
		border-radius: 50%;
	}

	.intent-mark.live {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.8);
	}

	.intent-mark.read {
		border: 2px solid var(--color-muted);
	}

	:global(.intent-label) {
		display: inline-block;
		min-width: 4.7rem;
		margin-right: 0.65rem;
		font-family: var(--font-mono);
		font-size: 0.62rem;
		font-weight: 500;
		letter-spacing: 0.07em;
		text-transform: uppercase;
	}

	:global(.intent-dry .intent-label) {
		color: var(--color-cyan);
	}

	:global(.intent-live .intent-label) {
		color: var(--color-orange);
	}

	:global(.intent-read .intent-label) {
		color: var(--color-muted);
	}

	:global(.intent-dry) {
		border-left: 2px solid var(--color-cyan);
		padding-left: 0.7rem;
	}

	:global(.intent-live) {
		border-left: 2px solid var(--color-orange);
		padding-left: 0.7rem;
	}

	:global(.intent-read) {
		border-left: 2px solid var(--color-muted);
		padding-left: 0.7rem;
	}

	.report-step {
		padding: clamp(1.5rem, 4vw, 2.5rem);
		border: 1px solid var(--color-control-border);
		background: var(--color-layer);
	}

	.report-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 2rem;
	}

	.report-grid h3 {
		margin-bottom: 0.75rem;
		font-family: var(--font-body);
		font-size: 1rem;
		font-weight: 600;
		letter-spacing: 0;
	}

	.report-grid li {
		padding-block: 0.35rem;
		font-size: 0.88rem;
	}

	.report-grid li::before {
		top: 0.85rem;
	}

	.report-grid > div:last-child li::before {
		width: 0.52rem;
		height: 1px;
		background: var(--color-muted);
		transform: none;
	}

	.report-actions {
		margin-top: 1.5rem;
	}

	.contributor-panel {
		display: grid;
		grid-template-columns: minmax(180px, 0.7fr) minmax(0, 1.3fr);
		gap: clamp(1.5rem, 5vw, 3.5rem);
		padding: clamp(1.5rem, 4vw, 2.5rem);
		border: 1px solid var(--color-border);
		background: var(--color-code-bg);
	}

	.contributor-panel :global(.terminal-frame),
	.profile-note {
		grid-column: 2;
	}

	@media (max-width: 880px) {
		.install-grid {
			grid-template-columns: 1fr;
		}

		.safety-rail {
			position: static;
		}
	}

	@media (max-width: 680px) {
		.lane-heading,
		.install-lane {
			align-items: flex-start;
			grid-template-columns: 1fr;
		}

		.lane-heading {
			flex-direction: column;
		}

		.lane-heading code {
			max-width: 100%;
			text-align: left;
		}

		.install-lane code {
			justify-self: stretch;
			text-align: left;
		}

		.report-grid,
		.contributor-panel {
			grid-template-columns: 1fr;
		}

		.contributor-panel :global(.terminal-frame),
		.profile-note {
			grid-column: 1;
		}
	}
</style>
