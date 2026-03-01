<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';

	let { children } = $props();

	const navLinks = [
		{ href: '/', label: 'Home' },
		{ href: '/packages', label: 'Packages' },
		{ href: '/stats', label: 'Stats' },
		{ href: '/about', label: 'About' }
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
				<span class="logo-text">Conary</span>
				<span class="logo-badge">Remi</span>
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
			<p>Powered by Conary Remi</p>
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
		border-bottom: 1px solid var(--color-border);
		background: var(--color-bg);
		position: sticky;
		top: 0;
		z-index: 50;
	}

	.header-inner {
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: 3.5rem;
	}

	.logo {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		text-decoration: none;
		color: var(--color-text);
	}

	.logo:hover {
		text-decoration: none;
	}

	.logo-text {
		font-size: 1.25rem;
		font-weight: 700;
		letter-spacing: -0.02em;
	}

	.logo-badge {
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 0.15em 0.5em;
		background: var(--color-primary);
		color: #fff;
		border-radius: var(--radius-sm);
	}

	.nav-links {
		display: flex;
		list-style: none;
		margin: 0;
		padding: 0;
		gap: 0.25rem;
	}

	.nav-links a {
		display: block;
		padding: 0.375rem 0.75rem;
		border-radius: var(--radius-sm);
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--color-text-secondary);
		text-decoration: none;
		transition: all 0.15s;
	}

	.nav-links a:hover {
		color: var(--color-text);
		background: var(--color-bg-secondary);
		text-decoration: none;
	}

	.nav-links a.active {
		color: var(--color-primary);
		background: var(--color-bg-secondary);
	}

	.site-footer {
		border-top: 1px solid var(--color-border);
		margin-top: 4rem;
	}

	.footer-inner {
		padding-top: 2rem;
		padding-bottom: 2rem;
		text-align: center;
	}

	.footer-inner p {
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
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
