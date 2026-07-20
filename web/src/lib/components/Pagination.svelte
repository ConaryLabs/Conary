<script lang="ts">
	let {
		page = 1,
		totalPages = 1,
		onnavigate
	}: {
		page?: number;
		totalPages?: number;
		onnavigate: (page: number) => void;
	} = $props();

	function getVisiblePages(current: number, total: number): (number | '…')[] {
		if (total <= 7) return Array.from({ length: total }, (_, i) => i + 1);
		const pages: (number | '…')[] = [1];
		if (current > 3) pages.push('…');
		for (let i = Math.max(2, current - 1); i <= Math.min(total - 1, current + 1); i++) pages.push(i);
		if (current < total - 2) pages.push('…');
		pages.push(total);
		return pages;
	}

	let visiblePages = $derived(getVisiblePages(page, totalPages));
</script>

{#if totalPages > 1}
	<nav class="pagination" aria-label="Package pages">
		<button class="page-btn" disabled={page <= 1} onclick={() => onnavigate(page - 1)}>← Previous</button>
		<div class="page-numbers">
			{#each visiblePages as visiblePage}
				{#if visiblePage === '…'}
					<span class="page-ellipsis">…</span>
				{:else}
					<button
						class="page-num"
						class:active={visiblePage === page}
						onclick={() => onnavigate(visiblePage)}
						aria-label="Page {visiblePage}"
						aria-current={visiblePage === page ? 'page' : undefined}
					>
						{visiblePage}
					</button>
				{/if}
			{/each}
		</div>
		<button class="page-btn" disabled={page >= totalPages} onclick={() => onnavigate(page + 1)}>Next →</button>
	</nav>
{/if}

<style>
	.pagination {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		margin: 2.5rem 0 0;
	}

	.page-numbers {
		display: flex;
		align-items: center;
		gap: 0.2rem;
	}

	.page-btn,
	.page-num {
		min-height: 42px;
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-sm);
		color: var(--color-mist);
		background: var(--color-layer);
		font-family: var(--font-mono);
		font-size: 0.7rem;
	}

	.page-btn {
		padding: 0.55rem 0.9rem;
	}

	.page-num {
		min-width: 42px;
		padding: 0.45rem;
	}

	.page-btn:hover:not(:disabled),
	.page-num:hover {
		color: var(--color-cyan);
		border-color: var(--color-cyan);
	}

	.page-num.active {
		color: var(--color-field);
		border-color: var(--color-cyan);
		background: var(--color-cyan);
	}

	.page-btn:disabled {
		cursor: not-allowed;
		opacity: 0.35;
	}

	.page-ellipsis {
		min-width: 2rem;
		color: var(--color-muted);
		text-align: center;
	}

	@media (max-width: 560px) {
		.page-numbers {
			display: none;
		}
	}
</style>
