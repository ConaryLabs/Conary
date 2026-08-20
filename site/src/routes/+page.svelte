<script lang="ts">
	import BoundaryDiagram from '$lib/components/BoundaryDiagram.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import TerminalFrame from '$lib/components/TerminalFrame.svelte';
	import { previewRelease } from '$lib/preview-release';
</script>

<PageMeta
	title="Conary — One reversible package model for Linux"
	description="Conary installs RPM, DEB, Arch, and CCS packages across Linux distributions through one inspectable and reversible package model."
	path="/"
/>

<section class="hero">
	<div class="container grid-12 hero-grid">
		<div class="hero-copy">
			<p class="eyebrow">A package system for Linux · tester loop paused</p>
			<h1>An RPM that installs on Ubuntu. A DEB that installs on Fedora.</h1>
			<p class="hero-lede">
				Conary keeps each package's exact RPM, Debian, or ALPM semantics, executes
				them against typed capabilities on the target host, and owns the resulting
				transaction and rollback — without invoking dnf, apt, or pacman.
			</p>
			<div class="button-row hero-actions">
				<a href="/install/" class="btn btn-primary">Read the current release status</a>
				<a href="/features/" class="btn btn-secondary">See what is proven</a>
			</div>
			<ul class="hero-meta" aria-label="Supported formats and hosts">
				<li>RPM · DEB · Arch · CCS</li>
				<li>Fedora 44 · Ubuntu 26.04 · Arch</li>
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

<section class="section thesis">
	<div class="container thesis-inner">
		<div class="thesis-head">
			<p class="eyebrow">The bet</p>
			<h2>A package format is a source input, not a wall.</h2>
		</div>

		<div class="thesis-body">
			<p>
				A package belongs to the distribution that built it. If the software you
				need ships as an RPM and you run Ubuntu, you wait for a Debian maintainer,
				reach for a container, or change distributions. The work of packaging the
				same software gets repeated once per ecosystem, and the cost lands on
				maintainers and on anyone whose distro is not the popular one.
			</p>
			<p>
				Conary's bet is that this boundary is mechanical, not fundamental. RPM,
				Debian, and ALPM each expose a finite, documented lifecycle ABI. Encode
				those exactly, express what a host provides as typed capabilities, and a
				package stops being the property of one distribution — while Conary, not
				the distro's package manager, owns the transaction and the way back.
			</p>
		</div>

		<p class="thesis-claim">
			Existing distro repositories become source inputs to one package engine.
		</p>

		<p class="thesis-bound">
			Today that holds for RPM, DEB, Arch, and CCS packages on Fedora 44, Ubuntu
			26.04 LTS, and Arch Linux. Everything below is labelled with how far it has
			actually been proven.
		</p>
	</div>
</section>

<section class="section vision">
	<div class="container">
		<div class="vision-heading">
			<div>
				<p class="eyebrow">The Conary idea</p>
				<h2 class="section-heading">One model, from package intent to a complete system.</h2>
			</div>
			<p class="section-copy">
				Distribution differences are inputs, not walls. Conary preserves each source
				format's lifecycle, dependency, version, payload, and configuration rules,
				then executes them against the capabilities of the target Linux system.
			</p>
		</div>

		<div class="vision-grid">
			<article>
				<span class="vision-status available">foundation available</span>
				<h3>Install across package-format boundaries</h3>
				<p>Install an RPM on Ubuntu, a DEB on Fedora, or an Arch package on either without delegating the transaction to dnf, apt, or pacman.</p>
			</article>
			<article>
				<span class="vision-status available">preview today</span>
				<h3>Bring the existing system along</h3>
				<p>Adopt packages already owned by dnf, apt, or pacman as a reversible migration bridge, then transfer authority only through an explicit takeover.</p>
			</article>
			<article>
				<span class="vision-status building">available · growing</span>
				<h3>Treat packages as verifiable content</h3>
				<p>Store Conary-owned files by content hash, resolve dependencies as a graph, and build, sign, verify, and inspect the native CCS format.</p>
			</article>
			<article>
				<span class="vision-status building">active machinery</span>
				<h3>Describe the system you want</h3>
				<p>Compare a declarative model with a running host and develop a path toward planned package changes and cross-distribution replatforming.</p>
			</article>
			<article>
				<span class="vision-status advanced">VM evidence</span>
				<h3>Select complete system generations</h3>
				<p>Build content-reusing EROFS artifacts, select a generation for the next boot, and make recovery a system-level operation rather than a hopeful undo.</p>
			</article>
			<article>
				<span class="vision-status horizon">service today · wider horizon</span>
				<h3>Share package infrastructure</h3>
				<p>Use Remi as the package metadata and conversion service today, while federation, third-party publishing, and broader automation mature.</p>
			</article>
			<a href="/features/" class="vision-link">
				<span>See the full feature map and maturity of each layer</span>
				<span aria-hidden="true">→</span>
			</a>
		</div>
	</div>
</section>

<section class="section evaluate">
	<div class="container grid-12">
		<div class="evaluate-heading">
			<p class="eyebrow">Source proof active · packaged tester loop paused</p>
			<h2 class="section-heading">Cross the package boundary deliberately.</h2>
			<p class="section-copy">
				The next public loop remains deliberately focused: inspect a foreign-format
				package, confirm its exact target capabilities, install it through Conary,
				and prove that query, update planning, removal, and rollback still agree.
				The published {previewRelease.tag} artifact is the current immutable release,
				but this loop resumes only after #110's separate ordinary-package corpus gate
				passes and an exact tester release is assigned.
			</p>
		</div>

		<ol class="capability-list">
			<li>
				<span class="capability-mark track" aria-hidden="true"></span>
				<div>
					<h3>Run a foreign package as its source format intended</h3>
					<p>Keep RPM, Debian, or ALPM lifecycle arguments and ordering while Conary supplies the target root, users, services, and other typed interfaces.</p>
				</div>
				<span class="capability-state">source exact</span>
			</li>
			<li>
				<span class="capability-mark inspect" aria-hidden="true"></span>
				<div>
					<h3>Inspect a Conary-owned package change</h3>
					<p>Sync Remi, resolve a supported package, and review the complete dry-run before approving a live install.</p>
				</div>
				<span class="capability-state">dry-run first</span>
			</li>
			<li>
				<span class="capability-mark cross" aria-hidden="true"></span>
				<div>
					<h3>Use one package engine on three hosts</h3>
					<p>Exercise the same RPM, DEB, Arch, and CCS pipeline on Fedora 44, Ubuntu 26.04 LTS, and Arch while retaining source-specific version and dependency rules.</p>
				</div>
				<span class="capability-state">matrix proof</span>
			</li>
		</ol>
	</div>
</section>

<section class="section section-band evidence">
	<div class="container grid-12">
		<div class="evidence-copy">
			<p class="eyebrow">Cross-distro transaction preview</p>
			<h2 class="section-heading">See the source ABI and target contract before apply.</h2>
			<p class="section-copy">
				Start with the documented dry-run. Conary shows the package's typed lifecycle,
				dependencies, payload, and required host capabilities before it mutates the
				selected root.
			</p>

			<div class="authority-summary">
				<div>
					<span class="summary-label">What changes</span>
					<p>Conary owns the installed files, state, lifecycle transaction, and rollback record.</p>
				</div>
				<div>
					<span class="summary-label">What stays source-native</span>
					<p>The RPM, Debian, or ALPM ABI—not command-name guesses or the host's package manager.</p>
				</div>
			</div>
		</div>

		<div class="evidence-terminal">
			<TerminalFrame title="cross-distro package sequence">
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install ./package.deb --dry-run</span></span>
					<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary install ./package.deb --yes</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary list package --info</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary update package --dry-run</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary remove package --yes</span></span>
			</TerminalFrame>
			<p class="evidence-note">Command-only illustration. Output varies by host; use the ordered install runbook to capture evidence.</p>
		</div>
	</div>
</section>

<section class="section section-band fit-check">
	<div class="container grid-12">
		<div class="fit-title">
			<p class="eyebrow">Honest fit check</p>
			<h2 class="section-heading">Evaluate package portability, then the migration path.</h2>
		</div>
		<div class="fit-copy">
			<p>
				apt, dnf, pacman, and Nix are mature systems with much larger ecosystems.
				Conary's bet is different: existing distro repositories become source inputs
				to one package engine, while adoption remains the bridge for the machine you
				already have.
			</p>
			<a href="/compare/" class="text-link">Compare the trade-offs <span aria-hidden="true">→</span></a>
		</div>
	</div>
</section>

<section class="section final-cta">
	<div class="container cta-box">
		<div>
			<p class="eyebrow">See the package bridge on a real host</p>
			<h2>Try a package your distro did not ship.</h2>
			<p>Use the pinned, checksum-verified preview on a disposable host and inspect the complete transaction before apply.</p>
		</div>
		<div class="button-row">
			<a href="/install/" class="btn btn-primary">Open the install guide</a>
			<a href="https://github.com/ConaryLabs/Conary" class="btn btn-secondary">View the source <span aria-hidden="true">↗</span></a>
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
		min-height: min(760px, calc(100svh - var(--header-height)));
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
		font-size: 0.72rem;
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

	.vision {
		border-bottom: 1px solid var(--color-border);
		background: var(--color-layer);
	}

	.vision-heading {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(300px, 0.72fr);
		gap: clamp(2rem, 7vw, 6rem);
		align-items: end;
		margin-bottom: clamp(2.5rem, 6vw, 4.5rem);
	}

	.vision-heading h2 {
		max-width: 15ch;
	}

	.vision-heading .section-copy {
		margin: 0;
	}

	.vision-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 1px;
		border: 1px solid var(--color-border);
		background: var(--color-border);
	}

	.vision-grid article,
	.vision-link {
		background: var(--color-field);
	}

	.vision-grid article {
		min-height: 248px;
		padding: clamp(1.35rem, 3vw, 2rem);
	}

	.vision-status {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		font-family: var(--font-mono);
		font-size: 0.65rem;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.vision-status::before {
		content: '';
		width: 0.52rem;
		height: 0.52rem;
		flex: 0 0 auto;
	}

	.vision-status.available,
	.vision-status.building {
		color: var(--color-cyan);
	}

	.vision-status.available::before {
		background: var(--color-cyan);
	}

	.vision-status.building::before {
		border: 1px solid var(--color-cyan);
	}

	.vision-status.advanced {
		color: var(--color-orange);
	}

	.vision-status.advanced::before {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.82);
	}

	.vision-status.horizon {
		color: var(--color-muted);
	}

	.vision-status.horizon::before {
		width: 0.65rem;
		height: 1px;
		background: var(--color-muted);
	}

	.vision-grid h3 {
		max-width: 20ch;
		margin: 2.6rem 0 0.75rem;
		font-family: var(--font-body);
		font-size: 1.15rem;
		font-weight: 600;
		letter-spacing: -0.015em;
	}

	.vision-grid p {
		margin: 0;
		color: var(--color-muted);
		font-size: 0.9rem;
		line-height: 1.65;
	}

	.vision-link {
		grid-column: 1 / -1;
		display: flex;
		align-items: center;
		justify-content: space-between;
		min-height: 64px;
		padding: 1rem 1.4rem;
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.78rem;
		text-decoration: none;
	}

	.vision-link:hover {
		color: var(--color-field);
		background: var(--color-cyan);
	}

	.hero-art {
		grid-column: 8 / -1;
		align-self: center;
		margin: 0;
		min-width: 0;
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
		/* Indented to the claim's rule: the boundary belongs to the claim above it,
		   not to the section. */
		margin: 1.25rem 0 0;
		padding-left: clamp(1rem, 2vw, 1.5rem);
		border-left: 3px solid transparent;
		color: var(--color-muted);
		font-size: 0.92rem;
	}

	.evaluate-heading {
		grid-column: 1 / span 5;
	}

	.capability-list {
		grid-column: 7 / -1;
		margin: 0;
		padding: 0;
		list-style: none;
		border-top: 1px solid var(--color-border);
	}

	.capability-list li {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		gap: 1rem;
		align-items: start;
		padding-block: 1.4rem;
		border-bottom: 1px solid var(--color-border);
	}

	.capability-list h3 {
		margin-bottom: 0.35rem;
		font-family: var(--font-body);
		font-size: 1.05rem;
		font-weight: 600;
		letter-spacing: 0;
	}

	.capability-list p {
		margin: 0;
		color: var(--color-muted);
		font-size: 0.9rem;
	}

	.capability-mark {
		width: 0.82rem;
		height: 0.82rem;
		margin-top: 0.32rem;
		border: 2px solid var(--color-cyan);
		border-radius: 50%;
	}

	.capability-mark.inspect {
		border-radius: 2px;
	}

	.capability-mark.cross {
		border-color: var(--color-orange);
		border-radius: 2px;
		transform: rotate(45deg) scale(0.8);
	}

	.capability-state {
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.68rem;
		white-space: nowrap;
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
		font-size: 0.72rem;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.authority-summary p {
		margin: 0.35rem 0 0;
		color: var(--color-muted);
		font-size: 0.9rem;
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
		font-size: 0.7rem;
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

	.text-link {
		font-family: var(--font-mono);
		font-size: 0.78rem;
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
		.vision-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.hero-copy {
			grid-column: 1 / span 6;
		}

		.hero-art {
			grid-column: 7 / -1;
		}

		.thesis-inner {
			grid-template-columns: 1fr;
		}

		.evaluate-heading,
		.evidence-copy {
			grid-column: 1 / span 5;
		}

		.capability-list,
		.evidence-terminal {
			grid-column: 6 / -1;
		}

		.fit-copy {
			grid-column: 7 / -1;
		}
	}

	@media (max-width: 760px) {
		.vision-heading {
			grid-template-columns: 1fr;
		}

		.vision-grid {
			grid-template-columns: 1fr;
		}

		.vision-link {
			grid-column: 1;
		}

		.hero-grid {
			min-height: auto;
			padding-top: 2.75rem;
			padding-bottom: 0;
		}

		.hero-copy,
		.hero-art,
		.evaluate-heading,
		.capability-list,
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

		.evaluate-heading,
		.evidence-copy,
		.fit-title {
			margin-bottom: 2rem;
		}

		.capability-list li {
			grid-template-columns: auto minmax(0, 1fr);
		}

		.capability-state {
			grid-column: 2;
		}

		.evidence-terminal {
			margin-top: 0.5rem;
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
