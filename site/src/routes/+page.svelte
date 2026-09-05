<script lang="ts">
	import BoundaryDiagram from '$lib/components/BoundaryDiagram.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import TerminalFrame from '$lib/components/TerminalFrame.svelte';
	import { commandRisk } from '$lib/command-risk';
	import { previewRelease, project } from '$lib/preview-release';
</script>

<PageMeta
	title="Conary — One reversible package model for Linux"
	description={previewRelease.announcementClaim}
	path="/"
/>

<section class="hero">
	<div class="container grid-12 hero-grid">
		<div class="hero-copy">
			<p class="eyebrow">Pre-alpha · cross-distro package manager for Linux</p>
			<h1>An RPM that installs on Ubuntu. A DEB that installs on Fedora.</h1>
			<p class="hero-lede">
				Conary installs RPM, DEB, and Arch packages on Fedora, Ubuntu, and Arch hosts.
				Each package keeps its source format's exact lifecycle, dependency, version, and
				configuration semantics; Conary owns the transaction and the rollback. It never
				hands the work to dnf, apt, or pacman.
			</p>
			<div class="button-row hero-actions">
				<a href="/install/" class="btn btn-primary">Install the preview</a>
				<a href="/features/" class="btn btn-secondary">See what works today</a>
			</div>
			<ul class="hero-meta" aria-label="Formats, hosts, and license">
				<li>RPM · DEB · Arch · CCS</li>
				<li>Fedora 44 · Ubuntu 26.04 LTS · Arch Linux</li>
				<li>x86_64 · Rust · {project.license}</li>
			</ul>
		</div>

		<figure class="hero-art mark-settle">
			<BoundaryDiagram />
			<figcaption class="visually-hidden">
				A source package keeps its RPM, Debian, or ALPM semantics while typed
				target capabilities feed one Conary-owned package transaction.
			</figcaption>
		</figure>
	</div>
</section>

<section class="section status" aria-labelledby="status-title">
	<div class="container">
		<h2 id="status-title" class="visually-hidden">Current status</h2>
		<dl class="status-grid">
			<div>
				<dt>Latest release</dt>
				<dd>
					<a href={previewRelease.releaseUrl}>{previewRelease.tag}</a>, an immutable GitHub
					release with checksums, a detached CCS signature, and a signed bootstrap manifest.
				</dd>
			</div>
			<div>
				<dt>Maturity</dt>
				<dd>
					Pre-alpha. Expect failures. Use a VM, a snapshot, or a host you can throw away.
					It is not a replacement for apt, dnf, or pacman on a machine you rely on.
				</dd>
			</div>
			<div>
				<dt>External testing</dt>
				<dd>
					{#if previewRelease.testerPinAssigned}
						Open: {previewRelease.tag} is the assigned tester release. The gate state is
						tracked in <a href={project.launchStatusUrl}>launch-status.json</a>.
					{:else}
						Not open yet. No release is assigned as tester authority; the remaining launch
						gates are tracked in the open in
						<a href={project.launchStatusUrl}>launch-status.json</a>.
					{/if}
				</dd>
			</div>
		</dl>
	</div>
</section>

<section class="section thesis">
	<div class="container thesis-inner">
		<div class="thesis-head">
			<p class="eyebrow">The bet</p>
			<h2>A package format is a source input, not a wall.</h2>
		</div>

		<div class="thesis-body">
			<p>
				A package belongs to the distribution that built it. If the software you need
				ships as an RPM and you run Ubuntu, you wait for a Debian maintainer, reach for
				a container, or change distributions. The same software gets packaged once per
				ecosystem, and the cost lands on maintainers and on anyone whose distro is not
				the popular one.
			</p>
			<p>
				Conary's bet is that this boundary is mechanical, not fundamental. RPM, Debian,
				and ALPM each expose a finite, documented lifecycle ABI. Encode those exactly,
				describe what a host provides as typed capabilities, and a package stops being
				the property of one distribution, while Conary, not the distro's package
				manager, owns the transaction and the way back.
			</p>
		</div>

		<p class="thesis-claim">
			Existing distro repositories become source inputs to one package engine.
		</p>

		<p class="thesis-bound">
			Today that holds for RPM, DEB, Arch, and CCS packages on Fedora 44, Ubuntu 26.04
			LTS, and Arch Linux, x86_64 only. Everything on this site is labelled with how far
			it has actually been proven.
		</p>
	</div>
</section>

<section class="section section-band contract">
	<div class="container">
		<div class="contract-heading">
			<p class="eyebrow">Who owns what</p>
			<h2 class="section-heading">Three parties, one transaction.</h2>
			<p class="section-copy">
				The whole design reduces to a division of authority. Nothing in it is inferred
				from script text, distro names, or the host package manager's opinion.
			</p>
		</div>

		<div class="contract-grid">
			<article>
				<span class="contract-label source">The source format owns</span>
				<h3>The package ABI</h3>
				<ul>
					<li>Lifecycle events, their arguments, and their ordering</li>
					<li>Dependency and version comparison rules</li>
					<li>Payload layout and metadata</li>
					<li>Configuration-file semantics</li>
				</ul>
			</article>
			<article>
				<span class="contract-label conary">Conary owns</span>
				<h3>The transaction</h3>
				<ul>
					<li>Install, update, and remove</li>
					<li>Rollback and the changeset record</li>
					<li>Content-addressed storage of installed files</li>
					<li>Generation publication on hosts that support it</li>
				</ul>
			</article>
			<article>
				<span class="contract-label target">The target supplies</span>
				<h3>Typed capabilities</h3>
				<ul>
					<li>Architecture, libc, and dynamic loader</li>
					<li>Init and service-manager interfaces</li>
					<li>Filesystem layout, users, and security policy</li>
					<li>Interpreters and helper contracts</li>
				</ul>
			</article>
		</div>

		<p class="contract-note">
			A package that needs a capability the host does not provide fails preflight before
			anything is mutated. A lifecycle form Conary has not modelled is a bug to fix, not
			a reason to guess.
		</p>
	</div>
</section>

<section class="section truth">
	<div class="container grid-12">
		<div class="truth-col works">
			<p class="eyebrow">What works today</p>
			<h2 class="section-heading">Working on the three supported hosts.</h2>
			<p class="truth-note">
				Proof runs in the integration harness, inside Fedora, Ubuntu, and Arch containers
				against a harness-served fixture repository, on the current tree; the
				ordinary-package gate passed after {previewRelease.tag} was cut. That release is immutable
				install-and-bootstrap evidence; the behaviour below is not a claim about the
				artifact itself.
			</p>
			<ul class="truth-list">
				<li>Installing RPM, DEB, and Arch artifacts through typed lifecycle, dependency, payload, and configuration contracts, on any of the three hosts.</li>
				<li>Installing native CCS packages and Remi-converted RPM, DEB, and Arch packages with Conary as package authority.</li>
				<li>Adopting packages already owned by dnf, apt, or pacman, and unadopting them without deleting anything.</li>
				<li>Atomic package-state changesets, history, and rollback-oriented state tracking.</li>
				<li>Immutable EROFS and composefs generations, plus raw, qcow2, and x86_64 UEFI ISO export, on hosts with the kernel and tooling.</li>
				<li>A signed release: checksummed packages, a detached CCS signature, and an Ed25519-signed bootstrap manifest.</li>
			</ul>
		</div>
		<div class="truth-col breaks">
			<p class="eyebrow">What will break</p>
			<h2 class="section-heading">Known edges, stated up front.</h2>
			<ul class="truth-list">
				<li>A package that needs a target capability the host does not provide fails exact preflight before mutation.</li>
				<li>Source lifecycle forms outside the implemented RPM, Debian, or ALPM ABI are bugs to model and test; Conary does not invent behaviour from command text.</li>
				<li>Security-only updates fail closed unless a repository declares trusted advisory metadata support.</li>
				<li>Native transaction-history import is not implemented.</li>
				<li>Non-x86_64 generation boot assets are still reserved.</li>
				<li>No SBOM or provenance sidecars are published for the current release.</li>
			</ul>
		</div>
	</div>
</section>

<section class="section section-band evidence">
	<div class="container grid-12">
		<div class="evidence-copy">
			<p class="eyebrow">{previewRelease.testerPinAssigned ? 'The bounded loop' : 'The bounded loop · inactive until a release is pinned'}</p>
			<h2 class="section-heading">Cross the format boundary, then prove you can come back.</h2>
			<p class="section-copy">
				Pick a source whose package format differs from the host. Every dry-run shows the
				package's typed lifecycle, dependencies, payload, and required host capabilities
				before anything is mutated. {commandRisk.rule} The
				<a href="/install/#confirmation">policy classes and known exceptions</a> are
				derived from <code>{commandRisk.source}</code>.
			</p>

			<div class="authority-summary">
				<div>
					<span class="summary-label">What changes</span>
					<p>Conary owns the installed files, state, lifecycle transaction, and rollback record.</p>
				</div>
				<div>
					<span class="summary-label">What stays source-native</span>
					<p>The RPM, Debian, or ALPM ABI, not command-name guesses or the host's package manager.</p>
				</div>
			</div>
		</div>

		<div class="evidence-terminal">
			{#if previewRelease.testerPinAssigned}
				<TerminalFrame title="cross-distro package loop">
					<span class="terminal-line"><span class="terminal-command">source=ubuntu-26.04  # on Fedora or Arch; use fedora-44 on Ubuntu</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --dry-run</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --yes</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary list htop --info</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary query depends htop</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary update htop --dry-run</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary remove htop --yes</span></span>
				</TerminalFrame>
				<p class="evidence-note">
					Commands only; output is not shown because it varies by host. A reproducible
					captured demo is open work, not something the site pretends to have.
				</p>
			{:else}
				<!-- The live loop is owned by the paused tester guide; while no release is
				     pinned the homepage shows only the non-mutating inspection commands. -->
				<TerminalFrame title="inspection only · tester loop inactive">
					<span class="terminal-line"><span class="terminal-command">source=ubuntu-26.04  # on Fedora or Arch; use fedora-44 on Ubuntu</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install htop --from "$source" --dry-run</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install ./package.deb --dry-run</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary list</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary update --dry-run</span></span>
				</TerminalFrame>
				<p class="evidence-note">
					Dry-run inspection only. The live install, update, and remove loop is owned by
					the tester guide, which is paused until a release is pinned; the
					<a href="/install/#tester-loop">install page</a> keeps its retained commands
					folded away. No output is shown because it varies by host.
				</p>
			{/if}
		</div>
	</div>
</section>

<section class="section fit-check">
	<div class="container grid-12">
		<div class="fit-title">
			<p class="eyebrow">Honest fit check</p>
			<h2 class="section-heading">Evaluate package portability first, migration second.</h2>
		</div>
		<div class="fit-copy">
			<p>
				apt, dnf, pacman, and Nix are mature systems with far larger ecosystems. Conary
				is not the mature choice today. Its bet is different: existing distro
				repositories become source inputs to one package engine, and adoption stays the
				bridge for the machine you already have.
			</p>
			<div class="button-row">
				<a href="/compare/" class="btn btn-secondary">Compare the operating models</a>
				<a href="/about/" class="btn btn-secondary">Read the project history</a>
			</div>
		</div>
	</div>
</section>

<section class="section final-cta">
	<div class="container cta-box">
		<div>
			<p class="eyebrow">Inspect it on a disposable host</p>
			<h2>Install the pre-alpha and read the plan before you apply it.</h2>
			<p>
				The bootstrap script verifies the signed release manifest and the package for
				your host before it touches anything, and previews by default.
				{#if previewRelease.testerPinAssigned}
					{previewRelease.tag} is the assigned external tester release.
				{:else}
					The external tester loop stays inactive until a release is pinned.
				{/if}
			</p>
		</div>
		<div class="button-row">
			<a href="/install/" class="btn btn-primary">Open the install guide</a>
			<a href={project.repoUrl} class="btn btn-secondary">View the source <span aria-hidden="true">↗</span></a>
		</div>
	</div>
</section>

<style>
	.hero {
		position: relative;
		overflow: hidden;
		border-bottom: 1px solid var(--color-border);
	}

	.hero-grid {
		min-height: min(720px, calc(100svh - var(--header-height) - 40px));
		align-items: center;
		padding-block: clamp(3rem, 6vw, 5rem);
	}

	.hero-copy {
		position: relative;
		z-index: 2;
		grid-column: 1 / span 6;
	}

	.hero h1 {
		max-width: 17ch;
		margin-bottom: 1.4rem;
		font-size: var(--step-hero);
		font-weight: 700;
		letter-spacing: var(--track-hero);
	}

	.hero-lede {
		max-width: 54ch;
		margin-bottom: 1.85rem;
		color: var(--color-mist);
		font-size: clamp(1rem, 1.5vw, 1.12rem);
		line-height: 1.7;
	}

	.hero-actions {
		margin-bottom: 2rem;
	}

	.hero-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 0.55rem 1.5rem;
		margin: 0;
		padding: 1rem 0 0;
		border-top: 1px solid var(--color-border);
		list-style: none;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: var(--text-label);
	}

	.hero-meta li {
		position: relative;
		padding-left: 0.9rem;
	}

	.hero-meta li::before {
		content: '';
		position: absolute;
		top: 0.48rem;
		left: 0;
		width: 0.34rem;
		height: 0.34rem;
		background: var(--color-orange);
		transform: rotate(45deg);
	}

	.hero-art {
		grid-column: 8 / -1;
		align-self: center;
		margin: 0;
		min-width: 0;
	}

	.status {
		padding-block: clamp(2rem, 4vw, 3rem);
		border-bottom: 1px solid var(--color-border);
		background: var(--color-layer);
	}

	.status-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: clamp(1.5rem, 4vw, 3rem);
		margin: 0;
	}

	.status-grid > div {
		padding-left: 1rem;
		border-left: 2px solid var(--color-border-strong);
	}

	.status-grid > div:nth-child(2) {
		border-left-color: var(--color-orange);
	}

	.status-grid dt {
		margin-bottom: 0.4rem;
		color: var(--color-ivory);
		font-family: var(--font-mono);
		font-size: var(--text-label);
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.status-grid dd {
		margin: 0;
		color: var(--color-mist);
		font-size: 0.95rem;
		line-height: 1.65;
	}

	.thesis {
		background: var(--color-code-bg);
		border-bottom: 1px solid var(--color-border);
	}

	.thesis-inner {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr);
		gap: clamp(2rem, 5vw, 4.5rem);
	}

	.thesis-head h2 {
		max-width: 16ch;
		margin: 0;
		font-size: var(--step-lead);
		font-weight: 700;
	}

	.thesis-body p {
		max-width: 62ch;
		margin: 0 0 1.15rem;
		color: var(--color-mist);
		font-size: clamp(1rem, 1.4vw, 1.08rem);
		line-height: 1.75;
	}

	.thesis-body p:last-child {
		margin-bottom: 0;
	}

	.thesis-claim {
		grid-column: 1 / -1;
		max-width: 30ch;
		margin: clamp(2.5rem, 5vw, 3.75rem) 0 0;
		padding-left: clamp(1rem, 2vw, 1.5rem);
		border-left: 3px solid var(--color-orange);
		color: var(--color-ivory);
		font-family: var(--font-display);
		font-size: var(--step-lead);
		font-weight: 700;
		letter-spacing: var(--track-display);
		line-height: 1.2;
		text-wrap: balance;
	}

	.thesis-bound {
		grid-column: 1 / -1;
		max-width: 72ch;
		margin: 1.25rem 0 0;
		padding-left: clamp(1rem, 2vw, 1.5rem);
		border-left: 3px solid transparent;
		color: var(--color-muted);
		font-size: 0.95rem;
	}

	.contract-heading {
		max-width: 720px;
		margin-bottom: clamp(2rem, 5vw, 3.5rem);
	}

	.contract-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 1px;
		border: 1px solid var(--color-border);
		background: var(--color-border);
	}

	.contract-grid article {
		padding: clamp(1.35rem, 3vw, 2rem);
		background: var(--color-field);
	}

	.contract-label {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		font-family: var(--font-mono);
		font-size: var(--text-label);
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.contract-label::before {
		content: '';
		width: 0.52rem;
		height: 0.52rem;
		flex: 0 0 auto;
	}

	.contract-label.source {
		color: var(--color-mist);
	}

	.contract-label.source::before {
		border: 1px solid var(--color-control-border);
	}

	.contract-label.conary {
		color: var(--color-orange);
	}

	.contract-label.conary::before {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.82);
	}

	.contract-label.target {
		color: var(--color-cyan);
	}

	.contract-label.target::before {
		background: var(--color-cyan);
	}

	.contract-grid h3 {
		margin: 1.4rem 0 0.75rem;
		font-family: var(--font-body);
		font-size: 1.2rem;
		font-weight: 600;
		letter-spacing: -0.015em;
	}

	.contract-grid ul {
		margin: 0;
		padding-left: 1.1rem;
		color: var(--color-muted);
		font-size: 0.93rem;
		line-height: 1.65;
	}

	.contract-grid li + li {
		margin-top: 0.35rem;
	}

	.contract-note {
		max-width: 72ch;
		margin: 1.5rem 0 0;
		color: var(--color-mist);
		font-size: 0.95rem;
	}

	.truth-col.works {
		grid-column: 1 / span 6;
	}

	.truth-col.breaks {
		grid-column: 7 / -1;
	}

	.truth-col .section-heading {
		max-width: 18ch;
	}

	.truth-note {
		max-width: 60ch;
		margin: 0;
		color: var(--color-muted);
		font-size: 0.93rem;
	}

	.truth-list {
		margin: 1.5rem 0 0;
		padding: 0;
		list-style: none;
		border-top: 1px solid var(--color-border);
	}

	.truth-list li {
		position: relative;
		padding: 0.9rem 0 0.9rem 1.3rem;
		border-bottom: 1px solid var(--color-border);
		color: var(--color-mist);
		font-size: 0.95rem;
		line-height: 1.6;
	}

	.truth-list li::before {
		content: '';
		position: absolute;
		top: 1.45rem;
		left: 0;
		width: 0.45rem;
		height: 0.45rem;
	}

	.works .truth-list li::before {
		background: var(--color-cyan);
	}

	.breaks .truth-list li::before {
		background: var(--color-orange);
		transform: rotate(45deg);
	}

	.evidence-copy {
		grid-column: 1 / span 5;
	}

	.authority-summary {
		display: grid;
		gap: 1.25rem;
		margin-top: 2.25rem;
	}

	.authority-summary > div {
		padding-left: 1rem;
		border-left: 2px solid var(--color-border-strong);
	}

	.authority-summary > div:first-child {
		border-left-color: var(--color-cyan);
	}

	.authority-summary > div:last-child {
		border-left-color: var(--color-orange);
	}

	.summary-label {
		color: var(--color-ivory);
		font-family: var(--font-mono);
		font-size: var(--text-label);
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.authority-summary p {
		margin: 0.35rem 0 0;
		color: var(--color-muted);
		font-size: 0.93rem;
	}

	.evidence-terminal {
		grid-column: 7 / -1;
		align-self: center;
		min-width: 0;
	}

	.evidence-note {
		margin: 0.85rem 0 0;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: var(--text-label);
		line-height: 1.6;
	}

	.fit-title {
		grid-column: 1 / span 6;
	}

	.fit-copy {
		grid-column: 8 / -1;
		align-self: end;
	}

	.fit-copy p {
		margin-bottom: 1.5rem;
		color: var(--color-mist);
		font-size: 1.05rem;
	}

	.final-cta {
		padding-bottom: 0;
	}

	.cta-box {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: 3rem;
		padding: clamp(2rem, 5vw, 4rem);
		border: 1px solid var(--color-border-strong);
		background: linear-gradient(120deg, var(--color-layer) 0 68%, rgb(70 199 211 / 8%) 68% 100%);
	}

	.cta-box h2 {
		max-width: 18ch;
		margin-bottom: 0.75rem;
		font-size: var(--step-lead);
	}

	.cta-box p:last-child {
		max-width: 56ch;
		margin: 0;
		color: var(--color-mist);
	}

	.cta-box .button-row {
		justify-content: flex-end;
		flex: 0 0 auto;
	}

	@media (max-width: 980px) {
		.hero-copy {
			grid-column: 1 / span 6;
		}

		.hero-art {
			grid-column: 7 / -1;
		}

		.thesis-inner {
			grid-template-columns: 1fr;
		}

		.contract-grid {
			grid-template-columns: 1fr;
		}

		.evidence-copy {
			grid-column: 1 / span 5;
		}

		.evidence-terminal {
			grid-column: 6 / -1;
		}

		.fit-copy {
			grid-column: 7 / -1;
		}
	}

	@media (max-width: 760px) {
		.hero-grid {
			min-height: auto;
			padding-top: 2.75rem;
			padding-bottom: 0;
		}

		.hero-copy,
		.hero-art,
		.truth-col.works,
		.truth-col.breaks,
		.evidence-copy,
		.evidence-terminal,
		.fit-title,
		.fit-copy {
			grid-column: 1;
		}

		.hero h1 {
			max-width: 20ch;
		}

		.hero-art {
			margin: 2.5rem 0 0;
		}

		.status-grid {
			grid-template-columns: 1fr;
		}

		.truth-col.works,
		.evidence-copy,
		.fit-title {
			margin-bottom: 2rem;
		}

		.cta-box {
			align-items: stretch;
			flex-direction: column;
			background: var(--color-layer);
		}

		.cta-box .button-row {
			justify-content: flex-start;
		}
	}
</style>
