<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		eyebrow,
		title,
		description,
		aside
	}: {
		eyebrow: string;
		title: string;
		description: string;
		aside?: Snippet;
	} = $props();
</script>

<header class="page-intro section">
	<div class="container grid-12">
		<div class="intro-title">
			<p class="eyebrow">{eyebrow}</p>
			<h1>{title}</h1>
		</div>
		<div class="intro-copy">
			<p>{description}</p>
			{#if aside}
				<div class="intro-aside">
					{@render aside()}
				</div>
			{/if}
		</div>
	</div>
</header>

<style>
	.page-intro {
		padding-top: clamp(4rem, 9vw, 7rem);
		padding-bottom: clamp(3rem, 6vw, 5rem);
		border-bottom: 1px solid var(--color-border);
	}

	.intro-title {
		grid-column: 1 / span 6;
	}

	.intro-title h1 {
		max-width: 19ch;
		margin-bottom: 0;
		font-size: var(--step-hero);
		font-weight: 700;
		letter-spacing: var(--track-hero);
	}

	.intro-copy {
		grid-column: 8 / -1;
		align-self: end;
		padding-bottom: 0.55rem;
	}

	.intro-copy > p {
		max-width: 42ch;
		margin-bottom: 0;
		color: var(--color-mist);
		font-size: clamp(1.05rem, 2vw, 1.25rem);
		line-height: 1.65;
	}

	.intro-aside {
		margin-top: 1.5rem;
	}

	@media (max-width: 760px) {
		.intro-title,
		.intro-copy {
			grid-column: 1;
		}

		.intro-title h1 {
			max-width: 100%;
		}

		.intro-copy {
			padding-bottom: 0;
		}
	}
</style>
