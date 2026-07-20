<script lang="ts">
	import PageMeta from '$lib/components/PageMeta.svelte';
	import SearchBar from '$lib/components/SearchBar.svelte';
	import PackageCard from '$lib/components/PackageCard.svelte';
	import { getStatsOverview, getPopularPackages } from '$lib/api';
	import type { StatsOverview, PopularPackage } from '$lib/types';

	let stats: StatsOverview | null = $state(null);
	let popular: PopularPackage[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	const distros = [
		{ id: 'fedora', label: 'Fedora', code: 'rpm', desc: 'RPM package metadata and public conversion results' },
		{ id: 'arch', label: 'Arch Linux', code: 'arch', desc: 'Rolling repository metadata and public conversion results' },
		{ id: 'ubuntu', label: 'Ubuntu', code: 'deb', desc: 'DEB package metadata and public conversion results' }
	];

	$effect(() => {
		loadData();
	});

	async function loadData() {
		try {
			const [statsData, popularData] = await Promise.all([
				getStatsOverview(),
				getPopularPackages(undefined, 12)
			]);
			stats = statsData;
			popular = popularData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load package index data';
		} finally {
			loading = false;
		}
	}

	function formatNumber(n: number): string {
		if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
		if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
		return String(n);
	}
</script>

<PageMeta
	title="Conary Package Index"
	description="Browse public package metadata and policy-gated conversion evidence served by Conary Remi."
	path="/"
/>

<section class="hero section">
	<div class="container hero-grid">
		<div class="hero-copy animate-in" style="--stagger: 0">
			<p class="eyebrow">Remi / public package index</p>
			<h1>Package evidence, across distributions.</h1>
			<p class="hero-subtitle">
				Search upstream package metadata, inspect version and dependency evidence, and see
				which artifacts have crossed Remi's public conversion policy boundary.
			</p>
			<SearchBar placeholder="Search packages across all distributions…" />
		</div>

		<div class="authority-panel animate-in" style="--stagger: 2" aria-label="Remi package flow">
			<div class="authority-head">
				<img src="/brand/conary-mark.svg" alt="" width="52" height="52" />
				<div>
					<span class="authority-code">public-index / v1</span>
					<strong>Remi evidence path</strong>
				</div>
			</div>
			<ol>
				<li>
					<span>01</span>
					<div><strong>Source metadata</strong><small>Fedora, Arch, and Ubuntu indexes</small></div>
				</li>
				<li>
					<span>02</span>
					<div><strong>Policy review</strong><small>Unsupported behavior stays refused or held</small></div>
				</li>
				<li>
					<span>03</span>
					<div><strong>Public result</strong><small>Metadata and approved CCS conversions</small></div>
				</li>
			</ol>
		</div>
	</div>
</section>

<section class="system-context section-band">
	<div class="container system-context-grid">
		<div class="system-context-copy">
			<p class="eyebrow">One layer in a larger system</p>
			<h2 class="section-heading">The index turns distribution differences into inspectable package evidence.</h2>
			<p class="section-copy">
				Conary's wider direction is a shared model for package intent, authority,
				builds, and complete systems. Remi supplies the distribution-aware metadata
				and policy boundary that lets those layers make grounded decisions.
			</p>
			<a href="https://conary.io/features/" class="text-link">Explore the Conary product direction <span aria-hidden="true">↗</span></a>
		</div>
		<ol class="system-steps" aria-label="How the package index supports Conary">
			<li>
				<span>01</span>
				<div><strong>Resolve for the real target</strong><small>Keep distro identity, versions, repositories, and policy visible.</small></div>
			</li>
			<li>
				<span>02</span>
				<div><strong>Publish only reviewed results</strong><small>Expose approved conversion evidence while unsupported behavior stays refused.</small></div>
			</li>
			<li>
				<span>03</span>
				<div><strong>Feed higher-level decisions</strong><small>Give package operations and system models evidence they can inspect.</small></div>
			</li>
		</ol>
	</div>
</section>

{#if stats}
	<section class="telemetry" aria-label="Package index statistics">
		<div class="container telemetry-grid">
			<div class="telemetry-intro">
				<span class="eyebrow">Live index telemetry</span>
				<span>Current API counters</span>
			</div>
			<div class="stat">
				<span class="stat-value">{formatNumber(stats.total_packages)}</span>
				<span class="stat-label">Packages</span>
			</div>
			<div class="stat">
				<span class="stat-value">{stats.total_distros}</span>
				<span class="stat-label">Distributions</span>
			</div>
			<div class="stat">
				<span class="stat-value">{formatNumber(stats.total_downloads)}</span>
				<span class="stat-label">Downloads</span>
			</div>
			<div class="stat">
				<span class="stat-value">{formatNumber(stats.total_converted)}</span>
				<span class="stat-label">CCS results</span>
			</div>
		</div>
	</section>
{/if}

<section class="section section-band">
	<div class="container">
		<div class="section-intro">
			<p class="eyebrow">Browse by source</p>
			<h2 class="section-heading">Distribution indexes</h2>
			<p class="section-copy">Start with the source repository whose package evidence you want to inspect.</p>
		</div>
		<div class="distro-grid">
			{#each distros as distro, i}
				<a href="/packages/{distro.id}" class="distro-card animate-in" style="--stagger: {4 + i}">
					<span class="distro-code">{distro.code}</span>
					<span class="distro-name">{distro.label}</span>
					<span class="distro-desc">{distro.desc}</span>
					<span class="distro-action">Open index <span aria-hidden="true">→</span></span>
				</a>
			{/each}
		</div>
	</div>
</section>

<section class="section">
	<div class="container">
		<div class="popular-head">
			<div>
				<p class="eyebrow">Observed demand</p>
				<h2 class="section-heading">Popular packages</h2>
			</div>
			<a href="/packages" class="btn btn-secondary">Browse all packages</a>
		</div>

		{#if loading}
			<p class="status-msg">Loading current package evidence…</p>
		{:else if error}
			<p class="status-msg error">{error}</p>
		{:else if popular.length > 0}
			<div class="package-grid">
				{#each popular as pkg, i}
					<div class="animate-in" style="--stagger: {8 + i}">
						<PackageCard
							name={pkg.name}
							distro={pkg.distro}
							version={pkg.version}
							description={pkg.description ?? ''}
							downloads={pkg.download_count}
							size={pkg.size}
						/>
					</div>
				{/each}
			</div>
		{:else}
			<p class="status-msg">No popularity data is available yet.</p>
		{/if}
	</div>
</section>

<style>
	.hero {
		position: relative;
		overflow: hidden;
	}

	.hero::before {
		content: '';
		position: absolute;
		inset: 0;
		background:
			linear-gradient(90deg, transparent 0 66%, rgb(70 199 211 / 3%) 66% 66.15%, transparent 66.15%),
			linear-gradient(0deg, transparent 0 72%, rgb(252 107 22 / 4%) 72% 72.2%, transparent 72.2%);
		pointer-events: none;
	}

	.hero-grid {
		position: relative;
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(340px, 0.8fr);
		align-items: center;
		gap: clamp(3rem, 8vw, 7rem);
	}

	.hero-copy h1 {
		max-width: 11ch;
		margin-bottom: 1.35rem;
		font-size: clamp(3.2rem, 7.8vw, 7.2rem);
		font-weight: 700;
		letter-spacing: -0.065em;
	}

	.hero-subtitle {
		max-width: 64ch;
		margin-bottom: 2rem;
		color: var(--color-mist);
		font-size: clamp(1.05rem, 1.7vw, 1.25rem);
	}

	.authority-panel {
		border: 1px solid var(--color-border-strong);
		background: rgb(16 29 48 / 82%);
	}

	.authority-head {
		display: flex;
		align-items: center;
		gap: 1rem;
		padding: 1.25rem;
		border-bottom: 1px solid var(--color-border);
	}

	.authority-head img {
		border-radius: 8px;
	}

	.authority-head div {
		display: flex;
		flex-direction: column;
	}

	.authority-code {
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.68rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.authority-head strong {
		font-family: var(--font-display);
		font-size: 1.1rem;
	}

	.authority-panel ol {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.authority-panel li {
		display: grid;
		grid-template-columns: 2.3rem 1fr;
		gap: 1rem;
		padding: 1.15rem 1.25rem;
		border-bottom: 1px solid var(--color-border);
	}

	.authority-panel li:last-child {
		border-bottom: 0;
	}

	.authority-panel li > span {
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.76rem;
	}

	.authority-panel li div {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}

	.authority-panel small {
		color: var(--color-muted);
		font-size: 0.78rem;
	}

	.system-context {
		padding-block: clamp(3rem, 7vw, 5.5rem);
		border-top: 1px solid var(--color-border);
	}

	.system-context-grid {
		display: grid;
		grid-template-columns: minmax(0, 0.9fr) minmax(340px, 1.1fr);
		gap: clamp(3rem, 8vw, 7rem);
		align-items: start;
	}

	.system-context-copy h2 {
		max-width: 17ch;
	}

	.system-context-copy .text-link {
		display: inline-flex;
		margin-top: 1rem;
		font-family: var(--font-mono);
		font-size: 0.76rem;
	}

	.system-steps {
		margin: 0;
		padding: 0;
		border-top: 1px solid var(--color-border);
		list-style: none;
	}

	.system-steps li {
		display: grid;
		grid-template-columns: 2.4rem minmax(0, 1fr);
		gap: 1rem;
		padding: 1.3rem 0;
		border-bottom: 1px solid var(--color-border);
	}

	.system-steps li > span {
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.72rem;
	}

	.system-steps li div {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	.system-steps strong {
		font-family: var(--font-display);
		font-size: 1rem;
	}

	.system-steps small {
		color: var(--color-muted);
		font-size: 0.82rem;
	}

	.telemetry {
		border-block: 1px solid var(--color-border);
		background: var(--color-code-bg);
	}

	.telemetry-grid {
		display: grid;
		grid-template-columns: 1.35fr repeat(4, minmax(0, 0.65fr));
	}

	.telemetry-intro,
	.stat {
		min-width: 0;
		padding: 1.25rem clamp(1rem, 2.5vw, 1.7rem);
		border-right: 1px solid var(--color-border);
	}

	.telemetry-grid > :last-child {
		border-right: 0;
	}

	.telemetry-intro {
		display: flex;
		flex-direction: column;
		justify-content: center;
	}

	.telemetry-intro .eyebrow {
		margin-bottom: 0.2rem;
	}

	.telemetry-intro > span:last-child {
		color: var(--color-muted);
		font-size: 0.78rem;
	}

	.stat {
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}

	.stat-value {
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: clamp(1.25rem, 2.4vw, 1.8rem);
		font-weight: 500;
	}

	.stat-label {
		color: var(--color-muted);
		font-size: 0.67rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.section-intro {
		margin-bottom: 2.5rem;
	}

	.distro-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		border: 1px solid var(--color-border);
	}

	.distro-card {
		display: flex;
		min-height: 245px;
		flex-direction: column;
		padding: 1.5rem;
		border-right: 1px solid var(--color-border);
		color: var(--color-ivory);
		background: var(--color-field);
		text-decoration: none;
		transition: color 150ms ease, background 150ms ease;
	}

	.distro-card:last-child {
		border-right: 0;
	}

	.distro-card:hover {
		color: var(--color-ivory);
		background: rgb(70 199 211 / 6%);
	}

	.distro-code {
		margin-bottom: auto;
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.72rem;
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}

	.distro-name {
		margin-bottom: 0.55rem;
		font-family: var(--font-display);
		font-size: 1.45rem;
		font-weight: 700;
	}

	.distro-desc {
		min-height: 3.2em;
		color: var(--color-mist);
		font-size: 0.87rem;
	}

	.distro-action {
		margin-top: 1.1rem;
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}

	.popular-head {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: 1.5rem;
		margin-bottom: 2rem;
	}

	.popular-head .section-heading {
		margin-bottom: 0;
	}

	.package-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 1px;
		padding: 1px;
		background: var(--color-border);
	}

	@media (max-width: 980px) {
		.hero-grid {
			grid-template-columns: 1fr;
		}

		.authority-panel {
			max-width: 680px;
		}

		.system-context-grid {
			grid-template-columns: 1fr;
		}

		.telemetry-grid {
			grid-template-columns: repeat(4, minmax(0, 1fr));
		}

		.telemetry-intro {
			grid-column: 1 / -1;
			border-right: 0;
			border-bottom: 1px solid var(--color-border);
		}

		.package-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}

	@media (max-width: 700px) {
		.distro-grid {
			grid-template-columns: 1fr;
		}

		.distro-card {
			min-height: 210px;
			border-right: 0;
			border-bottom: 1px solid var(--color-border);
		}

		.distro-card:last-child {
			border-bottom: 0;
		}

		.popular-head {
			align-items: flex-start;
			flex-direction: column;
		}

		.package-grid {
			grid-template-columns: 1fr;
		}
	}

	@media (max-width: 520px) {
		.telemetry-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.stat:nth-child(3) {
			border-right: 0;
		}

		.stat {
			border-bottom: 1px solid var(--color-border);
		}
	}
</style>
