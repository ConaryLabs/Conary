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

	function getVisiblePages(current: number, total: number): (number | '...')[] {
		if (total <= 7) {
			return Array.from({ length: total }, (_, i) => i + 1);
		}

		const pages: (number | '...')[] = [1];

		if (current > 3) {
			pages.push('...');
		}

		const start = Math.max(2, current - 1);
		const end = Math.min(total - 1, current + 1);

		for (let i = start; i <= end; i++) {
			pages.push(i);
		}

		if (current < total - 2) {
			pages.push('...');
		}

		pages.push(total);
		return pages;
	}

	let visiblePages = $derived(getVisiblePages(page, totalPages));
</script>

{#if totalPages > 1}
	<nav class="pagination" aria-label="Pagination">
		<button
			class="page-btn"
			disabled={page <= 1}
			onclick={() => onnavigate(page - 1)}
			aria-label="Previous page"
		>
			Previous
		</button>

		<div class="page-numbers">
			{#each visiblePages as p}
				{#if p === '...'}
					<span class="page-ellipsis">...</span>
				{:else}
					<button
						class="page-num"
						class:active={p === page}
						onclick={() => onnavigate(p)}
						aria-label="Page {p}"
						aria-current={p === page ? 'page' : undefined}
					>
						{p}
					</button>
				{/if}
			{/each}
		</div>

		<button
			class="page-btn"
			disabled={page >= totalPages}
			onclick={() => onnavigate(page + 1)}
			aria-label="Next page"
		>
			Next
		</button>
	</nav>
{/if}

<style>
	.pagination {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		margin: 2rem 0;
	}

	.page-numbers {
		display: flex;
		align-items: center;
		gap: 0.25rem;
	}

	.page-btn {
		padding: 0.5rem 1rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		background: var(--color-bg);
		color: var(--color-text);
		font-size: 0.875rem;
		transition: all 0.15s;
	}

	.page-btn:hover:not(:disabled) {
		border-color: var(--color-primary);
		color: var(--color-primary);
	}

	.page-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.page-num {
		min-width: 2.25rem;
		height: 2.25rem;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: none;
		color: var(--color-text);
		font-size: 0.875rem;
		transition: all 0.15s;
	}

	.page-num:hover {
		background: var(--color-card-bg);
	}

	.page-num.active {
		background: var(--color-primary);
		color: #fff;
		border-color: var(--color-primary);
	}

	.page-ellipsis {
		min-width: 2.25rem;
		text-align: center;
		color: var(--color-text-secondary);
	}
</style>
