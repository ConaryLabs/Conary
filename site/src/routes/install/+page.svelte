<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import TerminalFrame from '$lib/components/TerminalFrame.svelte';
	import { previewRelease, project } from '$lib/preview-release';
</script>

<PageMeta
	title="Install the Conary pre-alpha — Conary"
	description={`Install the ${previewRelease.tag} pre-alpha on a disposable Fedora 44, Ubuntu 26.04 LTS, or Arch Linux host through the signed bootstrap script, then run the bounded cross-distro package loop.`}
	path="/install/"
/>

<PageIntro
	eyebrow="Install · pre-alpha"
	title="Install the preview on a host you can afford to break."
	description={`The latest immutable release is ${previewRelease.tag}. It installs through a signed bootstrap script on Fedora 44, Ubuntu 26.04 LTS, and Arch Linux, x86_64 only. No release is assigned as external tester authority yet, so treat every run as an early look rather than a supported evaluation.`}
/>

<section class="install-section">
	<div class="container install-grid">
		<aside class="safety-rail" aria-labelledby="safety-title">
			<p class="eyebrow">Before you start</p>
			<h2 id="safety-title">Ground rules</h2>
			<ul>
				<li>Use a VM, a snapshot, or a host you do not need.</li>
				<li>Match Fedora 44, Ubuntu 26.04 LTS, or Arch Linux on x86_64.</li>
				<li>Read every script and command before running it.</li>
				<li>Prefer <code>--dry-run</code> first; every mutation needs <code>--yes</code>.</li>
				<li>Expect failures, and report them.</li>
				<li>Nothing here is a daily driver.</li>
			</ul>
			<a href={previewRelease.matrixUrl} class="btn btn-secondary">
				Read the release matrix <span aria-hidden="true">↗</span>
			</a>
		</aside>

		<div class="install-content">
			<section class="status-panel" aria-labelledby="status-panel-title">
				<p class="eyebrow">Where things stand</p>
				<h2 id="status-panel-title">Released, not yet open for external testing.</h2>
				<p>
					{previewRelease.tag} is a real, immutable, signed release, and the bounded loop
					below has been proven on all three hosts. What is missing is a tester pin: the
					project has not assigned any release as the one it asks strangers to evaluate,
					because the remaining launch gates are still open. You are welcome to try it; the
					result is early feedback, not a milestone completion.
				</p>
				<div class="button-row">
					<a href={previewRelease.releaseUrl} class="btn btn-secondary">
						Inspect the {previewRelease.tag} release <span aria-hidden="true">↗</span>
					</a>
					<a href={project.launchStatusUrl} class="btn btn-secondary">
						See the open launch gates <span aria-hidden="true">↗</span>
					</a>
				</div>
			</section>

			<section class="install-step" id="bootstrap">
				<div class="step-heading">
					<span class="step-number">01</span>
					<div>
						<h2>Download, read, and preview the bootstrap script</h2>
						<p>
							The script downloads the release manifest and its detached signature, verifies
							the Ed25519 signature against the embedded release key before it reads any
							selection field, picks the one package for this host, downloads it, and checks
							its size and SHA-256. Without <code>--apply</code> it only prints the plan.
						</p>
					</div>
				</div>

				<TerminalFrame title="download, inspect, preview">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">curl --proto '=https' --tlsv1.2 -fLO {previewRelease.bootstrapScriptUrl}</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">less install-conary-preview.sh</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">bash ./install-conary-preview.sh</span></span>
				</TerminalFrame>

				<div class="boundary-note step-note">
					<strong>Do not pipe the URL into a shell.</strong>
					Read the script. It is short, fails closed on anything it does not recognise, and
					refuses to run on a host or architecture outside the supported set.
				</div>
			</section>

			<section class="install-step" id="apply">
				<div class="step-heading">
					<span class="step-number">02</span>
					<div>
						<h2>Apply</h2>
						<p>
							Installation needs both flags. The script hands the verified package to the
							host's own package manager (dnf, apt-get, or pacman) to install Conary itself;
							the package's post-install hook initialises the system database and the
							built-in Fedora, Ubuntu, and Arch source feeds. That is the only job the host
							package manager gets: Conary never delegates its own package transactions to
							it afterwards.
						</p>
					</div>
				</div>

				<TerminalFrame title="live mutation">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">bash ./install-conary-preview.sh --apply --yes</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">conary --version</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary repo list</span></span>
				</TerminalFrame>

				<details class="manual-path">
					<summary>Prefer to verify by hand? Download one package and check it yourself.</summary>
					<p class="manual-intro">
						Create a clean directory, fetch only the package for this host together with
						<code>SHA256SUMS</code>, and confirm the checksum prints <strong>OK</strong> before
						installing it with the native package manager.
					</p>
					<TerminalFrame title="clean preview directory">
						<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">mkdir -p "{previewRelease.workDirectory}" && cd "{previewRelease.workDirectory}"</span></span>
						<span class="terminal-line"><span class="terminal-command">base="{previewRelease.downloadBaseUrl}"</span></span>
						<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">curl -fLO "$base/SHA256SUMS"</span></span>
					</TerminalFrame>
					<div class="download-grid">
						{#each previewRelease.targets as target}
							<article class="download-lane">
								<div class="lane-heading">
									<span>{target.name}</span>
									<code>{target.asset}</code>
								</div>
								<TerminalFrame title={`${target.name} download, verify, install`}>
									<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">curl -fLO "$base/{target.asset}"</span></span>
									<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sha256sum -c SHA256SUMS --ignore-missing</span></span>
									<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">{target.installCommand}</span></span>
								</TerminalFrame>
							</article>
						{/each}
					</div>
				</details>
			</section>

			<section class="install-step" id="tester-loop">
				<div class="step-heading">
					<span class="step-number">03</span>
					<div>
						<h2>Run the bounded cross-distro loop in order</h2>
						<p>
							Sync Remi, then choose a source whose package format differs from the host's.
							Review every dry-run before the command that carries <code>--yes</code>.
						</p>
					</div>
				</div>

				<div class="loop-legend" aria-label="Command intent legend">
					<span><i class="intent-mark dry" aria-hidden="true"></i>inspect first</span>
					<span><i class="intent-mark live" aria-hidden="true"></i>live mutation</span>
					<span><i class="intent-mark read" aria-hidden="true"></i>read-only check</span>
				</div>

				<TerminalFrame title="first cross-distro loop">
					<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary repo sync remi</span></span>
					<span class="terminal-line intent-read"><span class="intent-label">choose</span><span class="terminal-command">source=ubuntu-26.04  # on Fedora or Arch; use fedora-44 on Ubuntu</span></span>
					<span class="terminal-line intent-dry"><span class="intent-label">inspect</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --dry-run</span></span>
					<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --yes</span></span>
					<span class="terminal-line intent-read"><span class="intent-label">read</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary list htop --info</span></span>
					<span class="terminal-line intent-read"><span class="intent-label">read</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary query depends htop</span></span>
					<span class="terminal-line intent-dry"><span class="intent-label">inspect</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary update htop --dry-run</span></span>
					<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary remove htop --yes</span></span>
				</TerminalFrame>

				<div class="boundary-note step-note">
					<strong>Capability preflight is automatic and has no bypass.</strong>
					Conary validates the package's exact declared requirements against the host
					before mutation and rejects anything the host cannot satisfy. A local
					<code>.rpm</code>, <code>.deb</code>, or <code>.pkg.tar.zst</code> file works the
					same way: <code>sudo conary install ./package.deb --dry-run</code>.
				</div>
			</section>

			<section class="install-step" id="adopt">
				<div class="step-heading">
					<span class="step-number">04</span>
					<div>
						<h2>Optionally, let Conary see what the host already has</h2>
						<p>
							Adoption records packages that stay owned by dnf, apt, or pacman. It transfers
							nothing. Unadoption removes the tracking and deletes no files.
						</p>
					</div>
				</div>

				<TerminalFrame title="reversible adoption">
					<span class="terminal-line intent-dry"><span class="intent-label">inspect</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system adopt --system --dry-run</span></span>
					<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system adopt --system</span></span>
					<span class="terminal-line intent-read"><span class="intent-label">read</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system adopt --status</span></span>
					<span class="terminal-line intent-dry"><span class="intent-label">inspect</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system unadopt --all --dry-run</span></span>
					<span class="terminal-line intent-live"><span class="intent-label">live</span><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system unadopt --all --yes</span></span>
				</TerminalFrame>
			</section>

			<section class="install-step report-step" id="report-feedback">
				<div class="step-heading">
					<span class="step-number">05</span>
					<div>
						<h2>Report what happened, including the boring parts</h2>
						<p>
							An exact capability error, a lifecycle defect, or a slow first conversion is
							useful evidence. A loop that simply worked is evidence too. Keep the public
							report concise.
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
						Read the tester guide <span aria-hidden="true">↗</span>
					</a>
				</div>
			</section>

			<section class="contributor-panel" id="source-build">
				<div>
					<p class="eyebrow">Contributor path</p>
					<h2>Build from source</h2>
					<p>
						For development, not as a substitute for the packaged path. Requires Rust
						1.98.0 or newer, Git, and Linux; Conary does not build on macOS or Windows.
					</p>
				</div>

				<TerminalFrame title="developer build">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">git clone {project.cloneUrl}</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">cd Conary</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">cargo build -p conary</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo ./target/debug/conary system init</span></span>
				</TerminalFrame>

				<p class="profile-note">
					For an isolated, non-root development database, pass a writable
					<code>--db-path</code> and keep using the same path for every later command.
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
		font-size: 0.9rem;
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

	.status-panel {
		padding: clamp(1.5rem, 4vw, 2.5rem);
		margin-bottom: clamp(2.5rem, 6vw, 4rem);
		border: 1px solid var(--color-orange);
		background: var(--color-layer);
	}

	.status-panel h2 {
		margin-bottom: 0.75rem;
		font-size: var(--step-section);
	}

	.status-panel > p:not(.eyebrow) {
		max-width: 70ch;
		color: var(--color-mist);
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
		font-size: var(--text-label);
	}

	.step-heading h2,
	.contributor-panel h2 {
		margin-bottom: 0.65rem;
		font-size: var(--step-section);
	}

	.step-heading p,
	.contributor-panel > div > p:last-child,
	.profile-note,
	.manual-intro {
		max-width: 68ch;
		margin: 0;
		color: var(--color-mist);
	}

	.step-note,
	.profile-note {
		margin-top: 1.25rem;
	}

	.manual-path {
		margin-top: 1.5rem;
		padding: 1rem 1.1rem;
		border: 1px solid var(--color-border);
		background: rgb(16 29 48 / 45%);
	}

	.manual-path summary {
		color: var(--color-cyan);
		cursor: pointer;
		font-family: var(--font-mono);
		font-size: 0.8rem;
	}

	.manual-intro {
		margin: 1rem 0 !important;
	}

	.manual-intro strong {
		color: var(--color-ivory);
	}

	.download-grid {
		display: grid;
		gap: 1rem;
		margin-top: 1rem;
	}

	.download-lane {
		min-width: 0;
	}

	.lane-heading {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 1rem;
		margin-bottom: 0.6rem;
	}

	.lane-heading span {
		font-weight: 600;
	}

	.lane-heading code {
		max-width: 65%;
		overflow-wrap: anywhere;
		text-align: right;
	}

	.loop-legend {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem 1.25rem;
		margin-bottom: 0.85rem;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: var(--text-label);
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
		font-size: 0.7rem;
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
		font-size: 0.9rem;
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
		.lane-heading {
			align-items: flex-start;
			flex-direction: column;
		}

		.lane-heading code {
			max-width: 100%;
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
