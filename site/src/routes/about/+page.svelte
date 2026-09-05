<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import { project } from '$lib/preview-release';
</script>

<PageMeta
	title="About Conary — Conary"
	description="Conary is an independent, ground-up Rust reimplementation of an influential package-manager idea, built by Fieldmouse Works and licensed MIT OR Apache-2.0."
	path="/about/"
/>

<PageIntro
	eyebrow="Project context"
	title="An old packaging idea, rebuilt from scratch."
	description="Conary takes inspiration from a mid-2000s design that got several things right early, then starts again in Rust with a deliberately narrow public preview and a modern generation model."
/>

<section class="independence-band">
	<div class="container independence-grid">
		<span class="independence-mark" aria-hidden="true"></span>
		<div>
			<p class="eyebrow">Independent project</p>
			<p>
				This is a ground-up reimplementation in Rust, not a fork, port, resurrection, or continuation of the original
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
						The <a href="https://en.wikipedia.org/wiki/Conary">original Conary package manager</a>,
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
						and resolves dependencies before applying changes, without reusing a line of the original codebase.
					</p>
				</div>
			</li>
		</ol>
	</div>
</section>

<section class="section section-band who">
	<div class="container grid-12">
		<div class="who-title">
			<p class="eyebrow">Who makes it</p>
			<h2 class="section-heading">A Fieldmouse Works project.</h2>
		</div>
		<div class="who-copy">
			<p>
				Conary is built by <a href={project.orgUrl}>{project.orgName}</a>, the home for open
				tools aimed at closed, expensive, or restrictive software ecosystems. The source lives
				at <a href={project.repoUrl}>github.com/FieldmouseWorks/Conary</a>; the GitHub
				organization was previously named ConaryLabs, and old links redirect.
			</p>
			<p>
				It is a single-maintainer project, which is why the verification layer carries the
				weight a team's review would otherwise carry: cross-distro proof runs on real Fedora,
				Ubuntu, and Arch machines rather than mocks, and a documentation-truth check fails CI
				when the README, the roadmap, or this site claims something the code does not do.
			</p>
			<div id="licensing" class="licensing">
				<p class="eyebrow">Licensing</p>
				<p>
					The Conary client and every library crate are licensed
					<strong>{project.license}</strong>, so you may use either license. Remi, the
					hosted package service, is licensed <strong>{project.remiLicense}</strong>: if you
					run a modified Remi as a service, its users are entitled to the source. Releases
					published through v0.16.1 remain MIT.
				</p>
			</div>
		</div>
	</div>
</section>

<section class="section approach">
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
				Conary does not ask upstream maintainers to change their packages. RPM, DEB, and Arch
				inputs are converted into source-independent CCS transactions that retain the source
				package ABI and run against typed target capabilities, locally or through Remi.
			</p>
			<p>
				The first cross-distro loop installs a foreign-format artifact, inspects it,
				plans an update, and removes it. Adoption remains a separate migration path for
				packages already owned by the host package manager.
			</p>
		</div>
	</div>
</section>

<section class="section section-band architecture">
	<div class="container grid-12">
		<div class="architecture-heading">
			<p class="eyebrow">Architecture</p>
			<h2 class="section-heading">Package state, content, and generation artifacts stay distinct.</h2>
		</div>

		<dl class="architecture-list">
			<div>
				<dt>Format parsers</dt>
				<dd>RPM, DEB, and Arch parsers preserve exact lifecycle, dependency, version, payload, and configuration semantics in one source-independent model.</dd>
			</div>
			<div>
				<dt>Resolver</dt>
				<dd>resolvo provides SAT-based dependency resolution across conflicts, virtual provides, and typed dependencies.</dd>
			</div>
			<div>
				<dt>CAS layer</dt>
				<dd>Files are stored by hash rather than only by package, so identical content is kept once.</dd>
			</div>
			<div>
				<dt>Package changesets</dt>
				<dd>Package operations commit database and file state through explicit changesets rather than implying a bootable generation for every install.</dd>
			</div>
			<div>
				<dt>Generation artifacts</dt>
				<dd>The advanced path builds EROFS images and can integrate composefs and fs-verity on compatible hosts, with raw, qcow2, and ISO export.</dd>
			</div>
			<div>
				<dt>System model</dt>
				<dd>Desired package state can be declared and inspected for drift; live application remains a VM-only path.</dd>
			</div>
			<div>
				<dt>Remi</dt>
				<dd>The conversion and package-serving service: authenticated source ingestion, immutable catalogs, signing, and atomic activation.</dd>
			</div>
			<div>
				<dt>Bootstrap</dt>
				<dd>A staged pipeline builds from cross-tools through a complete system image, with experimental architecture targets beyond the packaged x86_64 lane.</dd>
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
			<div><dt>Language</dt><dd>Rust, Edition 2024 · one Cargo workspace, one synchronized release</dd></div>
			<div><dt>Filesystem</dt><dd>EROFS · composefs and fs-verity for generations where the kernel supports them</dd></div>
			<div><dt>Database</dt><dd>SQLite · explicit schema epoch with a rebuild boundary instead of migrations</dd></div>
			<div><dt>Hashing</dt><dd>SHA-256 · XXH3 · FastCDC content-defined chunking</dd></div>
			<div><dt>Signatures</dt><dd>Ed25519 for CCS artifacts and the release bootstrap manifest</dd></div>
			<div><dt>Compression</dt><dd>Zstd · Gzip · XZ</dd></div>
			<div><dt>Resolver</dt><dd>resolvo SAT solver</dd></div>
			<div><dt>Server</dt><dd>Axum · Tantivy full-text search</dd></div>
		</dl>
	</div>
</section>

<section class="section contribute">
	<div class="container contribute-box">
		<div>
			<p class="eyebrow">Contribute</p>
			<h2>Help turn preview evidence into a trustworthy package manager.</h2>
			<p>
				The repository runs unit, integration, harness, formatting, lint, documentation-truth, and release workflows.
				Open issues and the contributing guide are the entry points.
			</p>
		</div>
		<div class="button-row">
			<a href={project.repoUrl} class="btn btn-primary">GitHub <span aria-hidden="true">↗</span></a>
			<a href={project.contributingUrl} class="btn btn-secondary">Contributing guide <span aria-hidden="true">↗</span></a>
			<a href={project.issuesUrl} class="btn btn-secondary">Issues <span aria-hidden="true">↗</span></a>
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
	.who-title,
	.approach-title,
	.architecture-heading,
	.stack-heading {
		grid-column: 1 / span 5;
	}

	.timeline,
	.who-copy,
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
		font-size: var(--text-label);
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
	.who-copy p,
	.approach-copy p {
		margin: 0 0 0.85rem;
		color: var(--color-mist);
		font-size: 0.97rem;
		line-height: 1.72;
	}

	.who-copy p:last-child,
	.approach-copy p:last-child {
		margin-bottom: 0;
	}

	.licensing {
		margin-top: 1.5rem;
		padding: 1.1rem 1.2rem;
		border: 1px solid var(--color-border-strong);
		border-left: 3px solid var(--color-cyan);
		background: var(--color-code-bg);
		scroll-margin-top: 8rem;
	}

	.licensing .eyebrow {
		margin-bottom: 0.5rem;
		color: var(--color-cyan);
	}

	.licensing strong {
		color: var(--color-ivory);
		font-family: var(--font-mono);
		font-weight: 500;
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
		font-size: 0.78rem;
		font-weight: 500;
	}

	.architecture-list dd,
	.stack-list dd {
		margin: 0;
		color: var(--color-mist);
		font-size: 0.93rem;
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
		max-width: 20ch;
		margin-bottom: 0.8rem;
		font-size: var(--step-lead);
	}

	.contribute-box p:last-child {
		max-width: 60ch;
		margin: 0;
		color: var(--color-mist);
	}

	.contribute-box .button-row {
		justify-content: flex-end;
		flex: 0 0 auto;
	}

	@media (max-width: 820px) {
		.origins-heading,
		.timeline,
		.who-title,
		.who-copy,
		.approach-title,
		.approach-copy,
		.architecture-heading,
		.architecture-list,
		.stack-heading,
		.stack-list {
			grid-column: 1 / -1;
		}

		.origins-heading,
		.who-title,
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
