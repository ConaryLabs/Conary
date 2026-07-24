<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
</script>

<PageMeta
	title="About Conary — Conary"
	description="Learn how Conary reinterprets an influential package-manager idea as an independent, ground-up Rust project."
	path="/about/"
/>

<PageIntro
	eyebrow="Project context"
	title="A ground-up take on an old package-manager idea."
	description="Conary takes inspiration from a visionary mid-2000s design, then starts again in Rust with a deliberately narrower public preview and a modern generation model."
/>

<section class="independence-band">
	<div class="container independence-grid">
		<span class="independence-mark" aria-hidden="true"></span>
		<div>
			<p class="eyebrow">Independent project</p>
			<p>
				This is a ground-up reimplementation in Rust—not a fork, port, resurrection, or continuation of the original
				rPath Conary codebase. It is not affiliated with, endorsed by, or maintained by rPath, SAS, or the original Conary developers.
			</p>
		</div>
	</div>
</section>

<section class="section origins">
	<div class="container grid-12">
		<div class="origins-heading">
			<p class="eyebrow">Origins</p>
			<h2 class="section-heading">The ideas outlived the first implementation.</h2>
			<p class="section-copy">
				The name is carried forward as a tribute to the engineering that got there first.
			</p>
		</div>

		<ol class="timeline">
			<li>
				<span class="timeline-date">mid-2000s</span>
				<div>
					<h3>rPath builds the original Conary</h3>
					<p>
						The <a href="https://en.wikipedia.org/wiki/Conary_(package_manager)">original Conary package manager</a>,
						developed by the rPath team, pioneered content-addressable package storage, repository-level binary diffs,
						a SAT-based resolver, and rollback of system state.
					</p>
				</div>
			</li>
			<li>
				<span class="timeline-date">2011</span>
				<div>
					<h3>The implementation goes dormant</h3>
					<p>
						The Python 2 implementation was tied to rPath Linux and Foresight Linux. After SAS acquired rPath,
						the project went dormant while its package-management ideas remained relevant.
					</p>
				</div>
			</li>
			<li>
				<span class="timeline-date">now</span>
				<div>
					<h3>Conary starts again in Rust</h3>
					<p>
						The current project treats the filesystem as a content store, gives mutations explicit transaction and recovery boundaries,
						and resolves dependencies before applying changes—without reusing the original codebase.
					</p>
				</div>
			</li>
		</ol>
	</div>
</section>

<section class="section section-band approach">
	<div class="container grid-12">
		<div class="approach-title">
			<p class="eyebrow">The problem</p>
			<h2 class="section-heading">Linux package state is fragmented by distribution.</h2>
		</div>
		<div class="approach-copy">
			<p>
				Every distribution maintains its own package format, repositories, tools, and transaction model.
				Switching hosts means changing commands and expectations, while package availability and recovery behavior vary.
			</p>
			<p>
				Conary does not ask upstream maintainers to change their packages. Remi converts supported RPM, DEB, and Arch inputs
				into Conary's CCS format for the matching target, while public serving remains limited to conversion-safe artifacts.
			</p>
			<p>
				The first tester loop is narrower than the source tree: package-manager install, adoption, listing/search,
				dry-run updates, and unadoption. Generation builds remain a separate explicit path.
			</p>
		</div>
	</div>
</section>

<section class="section architecture">
	<div class="container grid-12">
		<div class="architecture-heading">
			<p class="eyebrow">Architecture</p>
			<h2 class="section-heading">Package state, content, and generation artifacts stay distinct.</h2>
		</div>

		<dl class="architecture-list">
			<div>
				<dt>CAS layer</dt>
				<dd>Files are stored by hash rather than only by package, allowing identical content to be deduplicated.</dd>
			</div>
			<div>
				<dt>Resolver</dt>
				<dd>resolvo provides SAT-based dependency resolution across conflicts, virtual provides, and typed dependencies.</dd>
			</div>
			<div>
				<dt>Format parsers</dt>
				<dd>Native RPM, DEB, and Arch parsers feed a shared metadata model while unsupported behavior can still be refused.</dd>
			</div>
			<div>
				<dt>Package changesets</dt>
				<dd>Package operations commit database and file state through explicit changesets rather than implying a bootable generation for every install.</dd>
			</div>
			<div>
				<dt>Generation artifacts</dt>
				<dd>The advanced path builds EROFS images and can integrate composefs and fs-verity on compatible hosts.</dd>
			</div>
			<div>
				<dt>System model</dt>
				<dd>Desired package state can be declared and inspected for drift; live application remains a VM-only follow-up.</dd>
			</div>
			<div>
				<dt>Delta work</dt>
				<dd>CAS chunking and generation-delta work remain broader roadmap items, not a size-reduction promise for the first tester loop.</dd>
			</div>
			<div>
				<dt>Bootstrap</dt>
				<dd>A staged pipeline builds from cross-tools through a complete system image, with experimental architecture targets beyond the packaged x86_64 tester lane.</dd>
			</div>
		</dl>
	</div>
</section>

<section class="section stack-section">
	<div class="container grid-12">
		<div class="stack-heading">
			<p class="eyebrow">Implementation</p>
			<h2 class="section-heading">The current stack</h2>
		</div>

		<dl class="stack-list">
			<div><dt>Language</dt><dd>Rust, Edition 2024 · 8-member Cargo workspace</dd></div>
			<div><dt>Filesystem</dt><dd>EROFS · optional composefs and fs-verity integration for generations</dd></div>
			<div><dt>Database</dt><dd>SQLite · schema version 78 · DB-first runtime state</dd></div>
			<div><dt>Hashing</dt><dd>SHA-256 · XXH128</dd></div>
			<div><dt>Compression</dt><dd>Zstd · Gzip · XZ</dd></div>
			<div><dt>Server</dt><dd>Axum · Tantivy full-text search</dd></div>
			<div><dt>Resolver</dt><dd>resolvo SAT solver</dd></div>
		</dl>
	</div>
</section>

<section class="section contribute">
	<div class="container contribute-box">
		<div>
			<p class="eyebrow">Contribute</p>
			<h2>Help turn preview evidence into a trustworthy package manager.</h2>
			<p>
				The repository includes unit, integration, harness, formatting, lint, documentation-truth, and release workflows.
				Open issues and the contributing guide are the current entry points.
			</p>
		</div>
		<div class="button-row">
			<a href="https://github.com/ConaryLabs/Conary" class="btn btn-primary">GitHub <span aria-hidden="true">↗</span></a>
			<a href="https://github.com/ConaryLabs/Conary/blob/main/CONTRIBUTING.md" class="btn btn-secondary">Contributing guide <span aria-hidden="true">↗</span></a>
			<a href="https://github.com/ConaryLabs/Conary/issues" class="btn btn-secondary">Issues <span aria-hidden="true">↗</span></a>
		</div>
	</div>
</section>

<style>
	.independence-band {
		padding: 1.6rem 0;
		border-bottom: 1px solid var(--color-border);
		background: rgb(252 107 22 / 6%);
	}

	.independence-grid {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		gap: 1.2rem;
		align-items: start;
		max-width: 960px;
	}

	.independence-mark {
		width: 1rem;
		height: 1rem;
		margin-top: 0.25rem;
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.8);
	}

	.independence-band .eyebrow {
		margin-bottom: 0.35rem;
	}

	.independence-band p:last-child {
		margin: 0;
		color: var(--color-mist);
	}

	.origins-heading,
	.approach-title,
	.architecture-heading,
	.stack-heading {
		grid-column: 1 / span 5;
	}

	.timeline,
	.approach-copy,
	.architecture-list,
	.stack-list {
		grid-column: 7 / -1;
	}

	.timeline {
		margin: 0;
		padding: 0;
		list-style: none;
		border-top: 1px solid var(--color-border);
	}

	.timeline li {
		display: grid;
		grid-template-columns: 110px minmax(0, 1fr);
		gap: 1rem;
		padding: 1.5rem 0;
		border-bottom: 1px solid var(--color-border);
	}

	.timeline-date {
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.72rem;
		text-transform: uppercase;
	}

	.timeline h3 {
		margin-bottom: 0.55rem;
		font-family: var(--font-body);
		font-size: 1.05rem;
		font-weight: 600;
		letter-spacing: 0;
	}

	.timeline p,
	.approach-copy p {
		margin: 0 0 0.85rem;
		color: var(--color-mist);
		font-size: 0.95rem;
		line-height: 1.72;
	}

	.approach-copy p:last-child {
		margin-bottom: 0;
	}

	.architecture-list,
	.stack-list {
		margin: 0;
	}

	.architecture-list > div {
		display: grid;
		grid-template-columns: minmax(150px, 0.7fr) minmax(0, 1.5fr);
		gap: 1.25rem;
		padding: 1.15rem 0;
		border-top: 1px solid var(--color-border);
	}

	.architecture-list > div:last-child {
		border-bottom: 1px solid var(--color-border);
	}

	.architecture-list dt,
	.stack-list dt {
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		font-weight: 500;
	}

	.architecture-list dd,
	.stack-list dd {
		margin: 0;
		color: var(--color-mist);
		font-size: 0.92rem;
	}

	.stack-section {
		padding-top: 0;
		border-top: 0 !important;
	}

	.stack-list {
		border: 1px solid var(--color-border-strong);
		background: var(--color-code-bg);
	}

	.stack-list > div {
		display: grid;
		grid-template-columns: 130px minmax(0, 1fr);
		gap: 1rem;
		padding: 0.9rem 1rem;
		border-bottom: 1px solid var(--color-border);
	}

	.stack-list > div:last-child {
		border-bottom: 0;
	}

	.contribute {
		padding-bottom: 0;
	}

	.contribute-box {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: 3rem;
		padding: clamp(2rem, 5vw, 4rem);
		border: 1px solid var(--color-border-strong);
		background: var(--color-layer);
	}

	.contribute-box h2 {
		max-width: 16ch;
		margin-bottom: 0.8rem;
		font-size: clamp(2rem, 4vw, 3.4rem);
		letter-spacing: -0.05em;
	}

	.contribute-box p:last-child {
		max-width: 60ch;
		margin: 0;
		color: var(--color-mist);
	}

	.contribute-box .button-row {
		justify-content: flex-end;
		flex: 0 0 auto;
		max-width: 310px;
	}

	@media (max-width: 820px) {
		.origins-heading,
		.timeline,
		.approach-title,
		.approach-copy,
		.architecture-heading,
		.architecture-list,
		.stack-heading,
		.stack-list {
			grid-column: 1 / -1;
		}

		.origins-heading,
		.approach-title,
		.architecture-heading,
		.stack-heading {
			margin-bottom: 2rem;
		}

		.contribute-box {
			align-items: stretch;
			flex-direction: column;
		}

		.contribute-box .button-row {
			justify-content: flex-start;
			max-width: none;
		}
	}

	@media (max-width: 520px) {
		.timeline li,
		.architecture-list > div,
		.stack-list > div {
			grid-template-columns: 1fr;
			gap: 0.45rem;
		}
	}
</style>
