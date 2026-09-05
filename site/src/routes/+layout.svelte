<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { previewRelease, project } from '$lib/preview-release';

	let { children } = $props();

	const navLinks = [
		{ href: '/', label: 'Home' },
		{ href: '/features/', label: 'Features' },
		{ href: '/install/', label: 'Install' },
		{ href: '/compare/', label: 'Compare' },
		{ href: '/about/', label: 'About' },
		{ href: '/contact/', label: 'Contact' }
	];

	function isActive(href: string): boolean {
		const current = page.url.pathname.replace(/\/+$/, '') || '/';
		const target = href.replace(/\/+$/, '') || '/';
		return current === target || (target !== '/' && current.startsWith(`${target}/`));
	}
</script>

<a class="skip-link" href="#main-content">Skip to main content</a>

<div class="app">
	<header class="site-header">
		<div class="container header-inner">
			<a href="/" class="brand" aria-label="Conary home">
				<img src="/brand/conary-mark.svg" alt="" width="38" height="38" />
				<span>Conary</span>
			</a>

			<nav class="site-nav" aria-label="Main navigation">
				<ul>
					{#each navLinks as link}
						<li>
							<a
								href={link.href}
								class="nav-link"
								class:active={isActive(link.href)}
								aria-current={isActive(link.href) ? 'page' : undefined}
							>
								{link.label}
							</a>
						</li>
					{/each}
				</ul>
			</nav>

			<a href={project.packagesUrl} class="packages-link">
				Packages <span aria-hidden="true">↗</span>
			</a>
		</div>
	</header>

	<!-- The one fact every page must carry: this is pre-alpha software. -->
	<aside class="status-strip" aria-label="Project status">
		<div class="container status-inner">
			<span class="status-item"><span class="status-dot" aria-hidden="true"></span>pre-alpha</span>
			<span class="status-item">
				latest release <a href={previewRelease.releaseUrl}>{previewRelease.tag}</a>
			</span>
			{#if previewRelease.testerState === 'assigned_guide_active'}
				<span class="status-item status-wide">external tester release pinned · loop open</span>
			{:else if previewRelease.testerState === 'assigned_guide_paused'}
				<span class="status-item status-wide">tester release assigned · loop not yet open</span>
			{:else}
				<span class="status-item status-wide">no external tester pin assigned</span>
			{/if}
			<a class="status-link" href="/install/">what that means</a>
		</div>
	</aside>

	<main id="main-content" tabindex="-1">
		{@render children()}
	</main>

	<footer class="site-footer">
		<div class="container footer-grid">
			<div class="footer-manifest">
				<a href="/" class="footer-brand">
					<img src="/brand/conary-mark.svg" alt="" width="42" height="42" />
					<span>Conary</span>
				</a>
				<p>
					A cross-distro package manager for Linux. RPM, DEB, and Arch packages keep
					their source semantics; Conary owns the transaction and the rollback.
				</p>
				<p class="footer-org">
					A <a href={project.orgUrl}>{project.orgName}</a> project.
					{project.orgTagline}
				</p>
			</div>

			<nav class="footer-nav" aria-label="Footer navigation">
				{#each navLinks as link}
					<a href={link.href}>{link.label}</a>
				{/each}
				<a href={project.packagesUrl}>Packages <span aria-hidden="true">↗</span></a>
				<a href={project.repoUrl}>GitHub <span aria-hidden="true">↗</span></a>
				<a href={project.discussionsUrl}>Discussions <span aria-hidden="true">↗</span></a>
				<a href={project.securityPolicyUrl}>Security policy <span aria-hidden="true">↗</span></a>
			</nav>
		</div>
		<div class="container footer-bottom">
			<span>
				Client and libraries <a href="/about/#licensing">{project.license}</a> · independent, ground-up
				Rust implementation
			</span>
			<a href="/about/">Project history and non-affiliation</a>
		</div>
	</footer>
</div>

<style>
	.app {
		display: flex;
		flex-direction: column;
		min-height: 100svh;
	}

	main {
		flex: 1;
	}

	.site-header {
		position: sticky;
		top: 0;
		z-index: 50;
		background: rgb(10 18 36 / 96%);
		border-bottom: 1px solid var(--color-border);
	}

	.header-inner {
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: clamp(1rem, 4vw, 3rem);
		min-height: var(--header-height);
	}

	.brand,
	.footer-brand {
		display: inline-flex;
		align-items: center;
		gap: 0.7rem;
		color: var(--color-ivory);
		font-family: var(--font-display);
		font-weight: 700;
		letter-spacing: -0.035em;
		text-decoration: none;
	}

	.brand {
		font-size: 1.15rem;
	}

	.brand img {
		border-radius: 7px;
	}

	.site-nav ul {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: clamp(0.15rem, 1vw, 0.5rem);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.nav-link {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 44px;
		padding: 0.5rem 0.72rem;
		color: var(--color-mist);
		font-size: 0.92rem;
		font-weight: 500;
		text-decoration: none;
	}

	.nav-link::after {
		content: '';
		position: absolute;
		bottom: 0.2rem;
		left: 50%;
		width: 0.43rem;
		height: 0.43rem;
		background: var(--color-orange);
		transform: translateX(-50%) rotate(45deg) scale(0);
		transition: transform 150ms ease;
	}

	.nav-link:hover,
	.nav-link.active {
		color: var(--color-ivory);
	}

	.nav-link.active::after {
		transform: translateX(-50%) rotate(45deg) scale(1);
	}

	.packages-link {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-height: 44px;
		padding: 0.55rem 0.85rem;
		border: 1px solid var(--color-control-border);
		border-radius: var(--radius-sm);
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.8rem;
		text-decoration: none;
	}

	.packages-link:hover {
		color: var(--color-field);
		background: var(--color-cyan);
		border-color: var(--color-cyan);
	}

	.status-strip {
		border-bottom: 1px solid var(--color-border);
		background: var(--color-code-bg);
		color: var(--color-mist);
		font-family: var(--font-mono);
		font-size: 0.76rem;
	}

	.status-inner {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.35rem 1.6rem;
		min-height: 40px;
		padding-block: 0.4rem;
	}

	.status-item {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		white-space: nowrap;
	}

	.status-dot {
		width: 0.5rem;
		height: 0.5rem;
		background: var(--color-orange);
		transform: rotate(45deg);
	}

	.status-strip a {
		color: var(--color-cyan);
	}

	.status-link {
		margin-left: auto;
	}

	.site-footer {
		margin-top: var(--section-space);
		border-top: 1px solid var(--color-border);
		background: var(--color-code-bg);
	}

	.footer-grid {
		display: grid;
		grid-template-columns: minmax(0, 1.3fr) minmax(0, 1fr);
		gap: 3rem;
		padding-block: 3.5rem;
	}

	.footer-brand {
		font-size: 1.2rem;
	}

	.footer-brand img {
		border-radius: 8px;
	}

	.footer-manifest p {
		max-width: 48ch;
		margin: 1rem 0 0;
		color: var(--color-muted);
	}

	.footer-org {
		font-size: 0.92rem;
	}

	.footer-nav {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		align-content: start;
		gap: 0.2rem 1.25rem;
	}

	.footer-nav a {
		min-height: 38px;
		padding-block: 0.45rem;
		color: var(--color-mist);
		font-size: 0.92rem;
		text-decoration: none;
	}

	.footer-nav a:hover {
		color: var(--color-cyan);
	}

	.footer-bottom {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		padding-block: 1.25rem;
		border-top: 1px solid var(--color-border);
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: 0.74rem;
	}

	.footer-bottom a {
		color: var(--color-muted);
	}

	.footer-bottom a:hover {
		color: var(--color-cyan);
	}

	@media (max-width: 760px) {
		.header-inner {
			grid-template-columns: 1fr auto;
			gap: 0.4rem 1rem;
			min-height: auto;
			padding-block: 0.6rem 0.45rem;
		}

		.site-nav {
			grid-column: 1 / -1;
			grid-row: 2;
			border-top: 1px solid var(--color-border);
		}

		.packages-link {
			grid-column: 2;
			grid-row: 1;
		}

		.site-nav ul {
			display: grid;
			grid-template-columns: repeat(6, minmax(0, 1fr));
			gap: 0;
		}

		.nav-link {
			padding-inline: 0.25rem;
			font-size: clamp(0.72rem, 2.8vw, 0.86rem);
		}

		.packages-link {
			padding-inline: 0.65rem;
		}

		.status-link {
			margin-left: 0;
		}

		.footer-grid {
			grid-template-columns: 1fr;
			gap: 2rem;
		}

		.footer-bottom {
			align-items: flex-start;
			flex-direction: column;
		}
	}

	@media (max-width: 380px) {
		.brand span {
			font-size: 1rem;
		}

		.brand img {
			width: 34px;
			height: 34px;
		}

		.packages-link {
			font-size: 0.72rem;
		}
	}
</style>
