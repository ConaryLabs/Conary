<script lang="ts">
	import PageMeta from '$lib/components/PageMeta.svelte';
	import TerminalFrame from '$lib/components/TerminalFrame.svelte';
	import { previewRelease } from '$lib/preview-release';
</script>

<PageMeta
	title="Conary — Reversible package experiments on Linux"
	description="Try Conary's limited Linux preview alongside dnf, apt, or pacman through reversible native-package adoption."
	path="/"
/>

<section class="hero">
	<div class="container grid-12 hero-grid">
		<div class="hero-copy">
			<p class="eyebrow">Limited preview · {previewRelease.tag}</p>
			<h1>Try Conary alongside the package manager you already trust.</h1>
			<p class="hero-lede">
				On Fedora 44, Ubuntu 26.04 LTS, or Arch, Conary can track native packages
				while dnf, apt, or pacman remains authoritative. Inspect the dry-run,
				then reverse the tracking without deleting package files.
			</p>
			<div class="button-row hero-actions">
				<a href="/install/" class="btn btn-primary">Run the limited preview</a>
				<a href="/features/" class="btn btn-secondary">See current boundaries</a>
			</div>
			<ul class="hero-meta" aria-label="Preview scope">
				<li>x86_64</li>
				<li>VM or non-critical host</li>
				<li>Native authority stays in place</li>
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

<section class="section evaluate">
	<div class="container grid-12">
		<div class="evaluate-heading">
			<p class="eyebrow">The first useful loop</p>
			<h2 class="section-heading">What you can evaluate now</h2>
			<p class="section-copy">
				The preview is intentionally narrow. It tests whether Conary can add useful
				package state without forcing a distribution migration first.
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

<section class="section beyond">
	<div class="container grid-12">
		<div class="beyond-intro">
			<p class="eyebrow">Beyond the first loop</p>
			<h2 class="section-heading">A path toward Conary-owned state, not a shortcut around consent.</h2>
		</div>

		<div class="beyond-grid">
			<article>
				<span class="article-mark cyan" aria-hidden="true"></span>
				<h3>Content-addressable storage</h3>
				<p>
					Conary-owned files are stored by content hash so identical content can be
					reused across packages instead of stored repeatedly.
				</p>
			</article>
			<article>
				<span class="article-mark orange" aria-hidden="true"></span>
				<h3>Optional system generations</h3>
				<p>
					The advanced path builds EROFS generation artifacts for explicit selection.
					It is separate from the basic package loop and should be exercised in a VM today.
				</p>
			</article>
			<a href="/features/" class="feature-link">
				<span>Capabilities and current boundaries</span>
				<span aria-hidden="true">→</span>
			</a>
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
			<p class="eyebrow">Ready to test the boundary?</p>
			<h2>Run the reversible preview on a VM.</h2>
			<p>Use the pinned release, verify its checksums, and start with the documented dry-run.</p>
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

	.beyond-intro {
		grid-column: 1 / span 5;
	}

	.beyond-grid {
		grid-column: 7 / -1;
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 1px;
		background: var(--color-border);
		border: 1px solid var(--color-border);
	}

	.beyond-grid article,
	.feature-link {
		background: var(--color-field);
	}

	.beyond-grid article {
		padding: clamp(1.4rem, 3vw, 2rem);
	}

	.article-mark {
		display: block;
		width: 0.9rem;
		height: 0.9rem;
		margin-bottom: 2.5rem;
	}

	.article-mark.cyan {
		border: 3px solid var(--color-cyan);
		border-radius: 50%;
	}

	.article-mark.orange {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.82);
	}

	.beyond-grid h3 {
		margin-bottom: 0.7rem;
		font-family: var(--font-body);
		font-size: 1.08rem;
		font-weight: 600;
		letter-spacing: 0;
	}

	.beyond-grid p {
		margin: 0;
		color: var(--color-muted);
		font-size: 0.9rem;
	}

	.feature-link {
		grid-column: 1 / -1;
		display: flex;
		align-items: center;
		justify-content: space-between;
		min-height: 62px;
		padding: 1rem 1.4rem;
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.78rem;
		text-decoration: none;
	}

	.feature-link:hover {
		color: var(--color-field);
		background: var(--color-cyan);
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
		.evidence-copy,
		.beyond-intro {
			grid-column: 1 / span 5;
		}

		.capability-list,
		.evidence-terminal,
		.beyond-grid {
			grid-column: 6 / -1;
		}

		.fit-copy {
			grid-column: 7 / -1;
		}
	}

	@media (max-width: 760px) {
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
		.beyond-intro,
		.beyond-grid,
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
		.beyond-intro,
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

		.beyond-grid {
			grid-template-columns: 1fr;
		}

		.feature-link {
			grid-column: 1;
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
