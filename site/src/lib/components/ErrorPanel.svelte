<script lang="ts">
	let {
		status,
		eyebrow,
		title,
		description
	}: {
		status: number;
		eyebrow: string;
		title: string;
		description: string;
	} = $props();
</script>

<section class="error-page">
	<div class="container error-grid">
		<div class="error-mark" aria-hidden="true">
			<span class="outer-c"></span>
			<span class="inner-c"></span>
			<span class="error-node"></span>
		</div>
		<div class="error-content">
			<p class="eyebrow">{status} · {eyebrow}</p>
			<h1>{title}</h1>
			<p>{description}</p>
			<div class="button-row">
				<a href="/" class="btn btn-primary">Return home</a>
				<a href="/install/" class="btn btn-secondary">Open install guide</a>
			</div>
		</div>
	</div>
</section>

<style>
	.error-page {
		display: grid;
		place-items: center;
		min-height: min(760px, calc(100svh - var(--header-height)));
		padding: var(--section-space) 0;
	}

	.error-grid {
		display: grid;
		grid-template-columns: minmax(220px, 0.8fr) minmax(0, 1.2fr);
		gap: clamp(2rem, 7vw, 6rem);
		align-items: center;
		max-width: 980px;
	}

	.error-mark {
		position: relative;
		width: min(100%, 360px);
		aspect-ratio: 1;
	}

	.outer-c,
	.inner-c {
		position: absolute;
		border-style: solid;
		border-right-color: transparent;
		border-radius: 50%;
		transform: rotate(-10deg);
	}

	.outer-c {
		inset: 8%;
		border-width: 34px;
		border-color: var(--color-ivory);
		border-right-color: transparent;
	}

	.inner-c {
		inset: 28%;
		border-width: 25px;
		border-color: var(--color-cyan);
		border-right-color: transparent;
	}

	.error-node {
		position: absolute;
		top: 46%;
		right: 9%;
		width: 2.4rem;
		height: 2.4rem;
		border-radius: 5px;
		background: var(--color-orange);
		transform: rotate(45deg);
	}

	.error-content h1 {
		max-width: 11ch;
		margin-bottom: 1rem;
		font-size: clamp(3rem, 7vw, 6rem);
		font-weight: 800;
		letter-spacing: -0.065em;
	}

	.error-content > p:not(.eyebrow) {
		max-width: 52ch;
		margin-bottom: 1.75rem;
		color: var(--color-mist);
		font-size: 1.05rem;
	}

	@media (max-width: 760px) {
		.error-page {
			min-height: auto;
		}
	}

	@media (max-width: 700px) {
		.error-grid {
			grid-template-columns: 1fr;
		}

		.error-mark {
			width: min(72vw, 290px);
			margin: 0 auto;
		}
	}
</style>
