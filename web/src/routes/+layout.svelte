<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';

	let { children } = $props();

	const mainSite = 'https://conary.io';
	const localLinks = [
		{ href: '/', label: 'Index' },
		{ href: '/packages', label: 'Browse' },
		{ href: '/stats', label: 'Stats' },
		{ href: '/about', label: 'About' }
	];

	const projectLinks = [
		{ href: `${mainSite}/features/`, label: 'Features' },
		{ href: `${mainSite}/install/`, label: 'Install' },
		{ href: `${mainSite}/compare/`, label: 'Compare' },
		{ href: `${mainSite}/about/`, label: 'Project' }
	];

	function isActive(href: string): boolean {
		const current = page.url.pathname.replace(/\/+$/, '') || '/';
		const target = href.replace(/\/+$/, '') || '/';
		if (target === '/') return current === '/';
		return current === target || current.startsWith(`${target}/`);
	}
</script>

<a class="skip-link" href="#main-content">Skip to main content</a>

<div class="app">
	<header class="site-header">
		<div class="container header-inner">
			<div class="brand-group">
				<a href={`${mainSite}/`} class="brand" aria-label="Conary project home">
					<img src="/brand/conary-mark.svg" alt="" width="38" height="38" />
					<span>Conary</span>
				</a>
				<a href="/" class="product-label" aria-label="Conary package index home">
					<span aria-hidden="true">/</span> package index
				</a>
			</div>

			<nav class="site-nav" aria-label="Package index navigation">
				<ul>
					{#each localLinks as link}
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

			<a href={`${mainSite}/`} class="project-link">
				Project site <span aria-hidden="true">↗</span>
			</a>
		</div>
	</header>

	<main id="main-content" tabindex="-1">
		{@render children()}
	</main>

	<footer class="site-footer">
		<div class="container footer-grid">
			<div class="footer-manifest">
				<a href="/" class="footer-brand">
					<img src="/brand/conary-mark.svg" alt="" width="42" height="42" />
					<span>Conary package index</span>
				</a>
				<p>Public package metadata and policy-gated conversion evidence served by Remi.</p>
			</div>

			<nav class="footer-nav" aria-label="Footer navigation">
				{#each localLinks as link}
					<a href={link.href}>{link.label}</a>
				{/each}
				{#each projectLinks as link}
					<a href={link.href}>{link.label} <span aria-hidden="true">↗</span></a>
				{/each}
				<a href="https://github.com/ConaryLabs/Conary">GitHub <span aria-hidden="true">↗</span></a>
			</nav>
		</div>
		<div class="container footer-bottom">
			<span>Remi conversion remains policy-gated during the limited preview.</span>
			<a href={`${mainSite}/install/`}>Open the tester runbook</a>
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
		grid-template-columns: minmax(max-content, 1fr) auto minmax(max-content, 1fr);
		align-items: center;
		gap: clamp(1rem, 3vw, 2.5rem);
		min-height: var(--header-height);
	}

	.brand-group {
		display: flex;
		align-items: center;
		min-width: 0;
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

	.brand img,
	.footer-brand img {
		border-radius: 7px;
	}

	.product-label {
		margin-left: 0.55rem;
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.72rem;
		font-weight: 500;
		text-decoration: none;
		white-space: nowrap;
	}

	.product-label span {
		margin-right: 0.35rem;
		color: var(--color-muted);
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
		font-size: 0.88rem;
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

	.project-link {
		justify-self: end;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-height: 44px;
		padding: 0.55rem 0.85rem;
		border: 1px solid var(--color-control-border);
		border-radius: var(--radius-sm);
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		text-decoration: none;
	}

	.project-link:hover {
		color: var(--color-field);
		background: var(--color-cyan);
		border-color: var(--color-cyan);
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

	.footer-manifest p {
		max-width: 48ch;
		margin: 1rem 0 0;
		color: var(--color-muted);
	}

	.footer-nav {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		align-content: start;
		gap: 0.2rem 1rem;
	}

	.footer-nav a {
		min-height: 38px;
		padding-block: 0.45rem;
		color: var(--color-mist);
		font-size: 0.84rem;
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
		font-size: 0.7rem;
	}

	.footer-bottom a {
		color: var(--color-muted);
	}

	.footer-bottom a:hover {
		color: var(--color-cyan);
	}

	@media (max-width: 900px) {
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

		.site-nav ul {
			display: grid;
			grid-template-columns: repeat(4, minmax(0, 1fr));
			gap: 0;
		}

		.nav-link {
			padding-inline: 0.25rem;
			font-size: clamp(0.7rem, 2.8vw, 0.82rem);
		}

		.footer-grid {
			grid-template-columns: 1fr;
			gap: 2rem;
		}
	}

	@media (max-width: 560px) {
		.product-label {
			display: none;
		}

		.project-link {
			padding-inline: 0.65rem;
			font-size: 0.68rem;
		}

		.footer-nav {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.footer-bottom {
			align-items: flex-start;
			flex-direction: column;
		}
	}
</style>
