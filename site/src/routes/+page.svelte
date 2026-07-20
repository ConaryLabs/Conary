<script lang="ts">
	import PageMeta from '$lib/components/PageMeta.svelte';
	import TerminalFrame from '$lib/components/TerminalFrame.svelte';
	import { previewRelease } from '$lib/preview-release';
</script>

<PageMeta
	title="Conary — One reversible package model for Linux"
	description="Conary is building portable, inspectable, and reversible package state across Linux distributions, from native-package adoption to declarative systems."
	path="/"
/>

<section class="hero">
	<div class="container grid-12 hero-grid">
		<div class="hero-copy">
			<p class="eyebrow">A package system for Linux · {previewRelease.tag}</p>
			<h1>Make Linux package state portable, inspectable, and reversible.</h1>
			<p class="hero-lede">
				Conary is building one model for packages and systems across distributions:
				adopt the machine you have, move authority only when you choose, and grow
				toward reproducible packages and selectable system state.
			</p>
			<div class="button-row hero-actions">
				<a href="/features/" class="btn btn-primary">Explore the product direction</a>
				<a href="/install/" class="btn btn-secondary">Try what works today</a>
			</div>
			<ul class="hero-meta" aria-label="Conary principles">
				<li>Fedora · Ubuntu · Arch</li>
				<li>Native authority by default</li>
				<li>Preview available now</li>
			</ul>
		</div>

		<figure class="hero-art mark-settle">
			<img
				src="/brand/conary-authority-rail.svg"
				alt=""
				width="820"
				height="640"
				aria-hidden="true"
			/>
			<div class="authority-labels" aria-hidden="true">
				<span class="authority-label native"><b>native authority</b><small>dnf / apt / pacman</small></span>
				<span class="authority-label tracked"><b>reversible tracking</b><small>adopt ⇄ unadopt</small></span>
				<span class="authority-label owned"><b>explicit opt-in</b><small>Conary-owned</small></span>
				<span class="authority-motto">reversible first · authority moves only when you choose</span>
			</div>
			<figcaption class="visually-hidden">
				The preview uses host-matching packages and repositories, then moves from
				native package-manager authority through reversible tracking. Conary-owned
				state remains a separate explicit choice.
			</figcaption>
		</figure>
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
				Linux distributions should not become interchangeable. Their package formats,
				policies, and native tools matter. Conary aims to add a shared control plane
				above those differences—without hiding who owns each change.
			</p>
		</div>

		<div class="vision-grid">
			<article>
				<span class="vision-status available">foundation available</span>
				<h3>Express package intent across distributions</h3>
				<p>Use shared package concepts while Conary resolves host-matching RPM, DEB, Arch, and CCS inputs with distribution-aware identity.</p>
			</article>
			<article>
				<span class="vision-status available">preview today</span>
				<h3>Adopt a system without reinstalling it</h3>
				<p>Bring existing native packages into view, keep dnf, apt, or pacman authoritative, and transfer ownership only through an explicit decision.</p>
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
			<p class="eyebrow">Available now · limited preview</p>
			<h2 class="section-heading">Start without starting over.</h2>
			<p class="section-copy">
				The first public loop is deliberately focused: prove Conary can add useful
				package state to a real host before asking users to trust the larger vision.
			</p>
		</div>

		<ol class="capability-list">
			<li>
				<span class="capability-mark track" aria-hidden="true"></span>
				<div>
					<h3>Track what is already installed</h3>
					<p>Adopt RPM, DEB, or pacman packages into Conary tracking while the native package manager keeps authority.</p>
				</div>
				<span class="capability-state">reversible</span>
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
					<h3>Use one preview workflow on three hosts</h3>
					<p>Exercise the same Conary concepts with host-matching packages and repositories on the current Fedora 44, Ubuntu 26.04 LTS, and Arch profiles.</p>
				</div>
				<span class="capability-state">limited scope</span>
			</li>
		</ol>
	</div>
</section>

<section class="section section-band evidence">
	<div class="container grid-12">
		<div class="evidence-copy">
			<p class="eyebrow">Authority-boundary preview</p>
			<h2 class="section-heading">See the boundary before you cross it.</h2>
			<p class="section-copy">
				Start with the documented dry-run. Adoption adds Conary tracking; it does not silently
				replace the native package database or remove the native package files.
			</p>

			<div class="authority-summary">
				<div>
					<span class="summary-label">What changes</span>
					<p>Conary records the package relationship and can report tracked state.</p>
				</div>
				<div>
					<span class="summary-label">What stays authoritative</span>
					<p>dnf, apt, or pacman continues to own the native package until you choose a later boundary.</p>
				</div>
			</div>
		</div>

		<div class="evidence-terminal">
			<TerminalFrame title="reversible adoption sequence">
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system adopt --system --dry-run</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system adopt --system</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary list</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system unadopt --all --dry-run</span></span>
				<span class="terminal-line"><span class="terminal-prompt">$</span><span class="terminal-command">sudo conary system unadopt --all --yes</span></span>
			</TerminalFrame>
			<p class="evidence-note">Command-only illustration. Output varies by host; use the ordered install runbook to capture evidence.</p>
		</div>
	</div>
</section>

<section class="section section-band fit-check">
	<div class="container grid-12">
		<div class="fit-title">
			<p class="eyebrow">Honest fit check</p>
			<h2 class="section-heading">Evaluate the migration path, not the package count.</h2>
		</div>
		<div class="fit-copy">
			<p>
				apt, dnf, pacman, and Nix are mature systems with much larger ecosystems.
				Conary is worth testing if reversible adoption, shared package-state concepts,
				and a gradual path toward Conary-owned state solve a real problem for you.
			</p>
			<a href="/compare/" class="text-link">Compare the trade-offs <span aria-hidden="true">→</span></a>
		</div>
	</div>
</section>

<section class="section final-cta">
	<div class="container cta-box">
		<div>
			<p class="eyebrow">See the idea on a real host</p>
			<h2>Start with the reversible layer.</h2>
			<p>The larger system is the direction. The pinned, checksum-verified preview is where you can begin safely today.</p>
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
		min-height: min(820px, calc(100svh - var(--header-height)));
		align-items: center;
		padding-block: clamp(4rem, 8vw, 7rem);
	}

	.hero-copy {
		position: relative;
		z-index: 2;
		grid-column: 1 / span 6;
	}

	.hero h1 {
		max-width: 10ch;
		margin-bottom: 1.5rem;
		font-size: clamp(3.2rem, 5vw, 4.8rem);
		font-weight: 800;
		letter-spacing: -0.065em;
	}

	.hero-lede {
		max-width: 60ch;
		margin-bottom: 2rem;
		color: var(--color-mist);
		font-size: clamp(1.05rem, 1.7vw, 1.2rem);
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
		position: relative;
		grid-column: 7 / -1;
		width: min(60vw, 760px);
		margin: 0 -8vw 0 -2rem;
	}

	.hero-art img {
		width: 100%;
		height: auto;
	}

	.authority-label {
		position: absolute;
		display: flex;
		flex-direction: column;
		gap: 0.18rem;
		font-family: var(--font-mono);
		line-height: 1.25;
	}

	.authority-label b {
		color: var(--color-ivory);
		font-size: clamp(0.62rem, 1.2vw, 0.82rem);
		font-weight: 500;
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}

	.authority-label small {
		color: var(--color-muted);
		font-size: clamp(0.56rem, 1vw, 0.72rem);
	}

	.authority-label.native {
		top: 34%;
		left: 51%;
	}

	.authority-label.tracked {
		top: 50%;
		left: 70%;
	}

	.authority-label.tracked b {
		color: var(--color-cyan);
	}

	.authority-label.owned {
		top: 62%;
		left: 70%;
	}

	.authority-label.owned b {
		color: var(--color-orange);
	}

	.authority-motto {
		position: absolute;
		bottom: 7%;
		left: 6%;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: clamp(0.52rem, 1vw, 0.68rem);
		letter-spacing: 0.08em;
		text-transform: uppercase;
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
		max-width: 14ch;
		margin-bottom: 0.75rem;
		font-size: clamp(2rem, 4vw, 3.5rem);
		letter-spacing: -0.045em;
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
			width: 720px;
			margin-right: -360px;
			opacity: 0.72;
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
			padding-top: 4.5rem;
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
			max-width: 10ch;
			font-size: clamp(2.65rem, 12vw, 3.7rem);
		}

		.hero-art {
			width: calc(100% + 5rem);
			margin: 1rem -3rem -2rem -2rem;
			opacity: 1;
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
