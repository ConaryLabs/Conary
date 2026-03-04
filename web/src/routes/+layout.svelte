<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';

	let { children } = $props();

	const navLinks = [
		{ href: '/', label: 'Home' },
		{ href: '/install', label: 'Install' },
		{ href: '/compare', label: 'Compare' },
		{ href: '/packages', label: 'Packages' },
		{ href: '/stats', label: 'Stats' }
	];

	function isActive(href: string): boolean {
		if (href === '/') return page.url.pathname === '/';
		return page.url.pathname.startsWith(href);
	}
</script>

<div class="app">
	<header class="site-header">
		<div class="container header-inner">
			<a href="/" class="logo">
				<span class="logo-text">conary</span>
				<span class="logo-sep">/</span>
				<span class="logo-badge">remi</span>
			</a>
			<nav aria-label="Main navigation">
				<ul class="nav-links">
					{#each navLinks as link}
						<li>
							<a href={link.href} class:active={isActive(link.href)}>
								{link.label}
							</a>
						</li>
					{/each}
				</ul>
			</nav>
		</div>
	</header>

	<main>
		{@render children()}
	</main>

	<footer class="site-footer">
		<div class="container footer-inner">
			<div class="footer-links">
				<a href="/">Home</a>
				<a href="/install">Install</a>
				<a href="/compare">Compare</a>
				<a href="/packages">Packages</a>
				<a href="/stats">Stats</a>
				<a href="/about">About</a>
				<a href="https://github.com/ConaryLabs/Conary" target="_blank" rel="noopener noreferrer">GitHub</a>
			</div>
			<div class="footer-bottom">
				<span class="footer-prompt">$</span>
				<span class="footer-text">powered by conary remi</span>
			</div>
		</div>
	</footer>
</div>

<style>
	.app {
		display: flex;
		flex-direction: column;
		min-height: 100vh;
	}

	main {
		flex: 1;
	}

	.site-header {
		position: sticky;
		top: 0;
		z-index: 50;
		background: rgba(12, 15, 20, 0.85);
		backdrop-filter: blur(12px);
		-webkit-backdrop-filter: blur(12px);
		border-bottom: 1px solid var(--color-border);
	}

	.header-inner {
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: var(--header-height);
	}

	.logo {
		display: flex;
		align-items: baseline;
		gap: 0;
		text-decoration: none;
		color: var(--color-text);
	}

	.logo:hover {
		text-decoration: none;
		color: var(--color-text);
	}

	.logo-text {
		font-family: var(--font-display);
		font-size: 1.25rem;
		font-weight: 700;
		letter-spacing: -0.02em;
	}

	.logo-sep {
		margin: 0 0.25rem;
		color: var(--color-text-muted);
		font-weight: 300;
	}

	.logo-badge {
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-accent);
	}

	.nav-links {
		display: flex;
		list-style: none;
		margin: 0;
		padding: 0;
		gap: 0.125rem;
	}

	.nav-links a {
		display: block;
		padding: 0.375rem 0.875rem;
		border-radius: var(--radius-sm);
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--color-text-secondary);
		text-decoration: none;
		transition: color 0.15s, background 0.15s;
	}

	.nav-links a:hover {
		color: var(--color-text);
		background: var(--color-surface);
		text-decoration: none;
	}

	.nav-links a.active {
		color: var(--color-accent);
		background: var(--color-accent-subtle);
	}

	.site-footer {
		border-top: 1px solid var(--color-border);
		margin-top: 4rem;
	}

	.footer-inner {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 1.25rem;
		padding-top: 2.5rem;
		padding-bottom: 2.5rem;
	}

	.footer-links {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem;
		justify-content: center;
	}

	.footer-links a {
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		padding: 0.25rem 0.625rem;
		border-radius: var(--radius-sm);
		transition: color 0.15s;
	}

	.footer-links a:hover {
		color: var(--color-accent);
	}

	.footer-bottom {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.footer-prompt {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-accent);
		font-weight: 500;
	}

	.footer-text {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-text-muted);
	}

	@media (max-width: 640px) {
		.header-inner {
			flex-direction: column;
			height: auto;
			padding-top: 0.75rem;
			padding-bottom: 0.75rem;
			gap: 0.5rem;
		}
	}
</style>
