<svelte:head>
	<title>Page not found - Conary</title>
	<meta name="description" content="The requested Conary page could not be found." />
	<meta name="robots" content="noindex" />
</svelte:head>

<section class="error-page">
	<div class="container error-content">
		<p class="error-code">404</p>
		<h1>Page not found</h1>
		<p>The page you requested is not part of the Conary site.</p>
		<a href="/" class="btn btn-primary">Return home</a>
	</div>
</section>

<style>
	.error-page {
		padding: 7rem 0;
	}

	.error-content {
		max-width: 620px;
		text-align: center;
	}

	.error-code {
		margin: 0 0 0.5rem;
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--color-accent);
	}

	h1 {
		margin-bottom: 0.75rem;
		font-family: var(--font-display);
		font-size: 2.75rem;
	}

	.error-content > p:not(.error-code) {
		margin: 0 0 1.5rem;
		color: var(--color-text-secondary);
	}
</style>
