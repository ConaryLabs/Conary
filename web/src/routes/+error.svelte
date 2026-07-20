<script lang="ts">
	import { page } from '$app/state';
	import PageMeta from '$lib/components/PageMeta.svelte';

	let status = $derived(page.status || 500);
	let title = $derived(status === 404 ? 'Package route not found' : 'Package index unavailable');
	let message = $derived(
		status === 404
			? 'The requested package-index route is not part of the current public map.'
			: 'The package index could not complete this request. Try the index again or return to the project site.'
	);
</script>

<PageMeta title={`${status} · ${title} — Conary`} description={message} noindex />

<section class="error-page">
	<div class="container error-grid">
		<div class="error-mark" aria-hidden="true">
			<img src="/brand/conary-mark.svg" alt="" width="180" height="180" />
		</div>
		<div class="error-content">
			<p class="eyebrow">{status} · package route</p>
			<h1>{title}</h1>
			<p>{message}</p>
			<div class="button-row">
				<a href="/" class="btn btn-primary">Open package index</a>
				<a href="https://conary.io/" class="btn btn-secondary">Return to Conary</a>
			</div>
		</div>
	</div>
</section>

<style>
	.error-page {
		padding-block: clamp(5rem, 12vw, 9rem);
	}

	.error-grid {
		display: grid;
		grid-template-columns: minmax(180px, 0.65fr) minmax(0, 1.35fr);
		align-items: center;
		gap: clamp(2rem, 7vw, 6rem);
	}

	.error-mark {
		display: grid;
		place-items: center;
		min-height: 260px;
		border: 1px solid var(--color-border);
		background: var(--color-layer);
	}

	.error-mark img {
		width: min(70%, 180px);
	}

	h1 {
		max-width: 12ch;
		margin-bottom: 1rem;
		font-size: clamp(2.6rem, 7vw, 5.4rem);
		letter-spacing: -0.055em;
	}

	.error-content > p:not(.eyebrow) {
		max-width: 58ch;
		margin-bottom: 2rem;
		color: var(--color-mist);
	}

	.button-row {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
	}

	@media (max-width: 700px) {
		.error-grid {
			grid-template-columns: 1fr;
		}

		.error-mark {
			min-height: 190px;
		}
	}
</style>
