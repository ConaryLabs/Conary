<script lang="ts">
	let {
		selected = 'all',
		onselect
	}: {
		selected?: string;
		onselect: (distro: string) => void;
	} = $props();

	const distros = [
		{ id: 'all', label: 'All' },
		{ id: 'fedora', label: 'Fedora' },
		{ id: 'arch', label: 'Arch' },
		{ id: 'ubuntu', label: 'Ubuntu' }
	];
</script>

<div class="distro-selector" role="tablist" aria-label="Distribution filter">
	{#each distros as d}
		<button
			role="tab"
			aria-selected={selected === d.id}
			class:active={selected === d.id}
			onclick={() => onselect(d.id)}
		>
			{d.label}
		</button>
	{/each}
</div>

<style>
	.distro-selector {
		display: flex;
		gap: 0.25rem;
		background: var(--color-bg-secondary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		padding: 0.25rem;
	}

	button {
		padding: 0.5rem 1.25rem;
		border: none;
		border-radius: var(--radius-sm);
		background: none;
		color: var(--color-text-secondary);
		font-size: 0.875rem;
		font-weight: 500;
		transition: all 0.15s;
	}

	button:hover {
		color: var(--color-text);
		background: var(--color-bg);
	}

	button.active {
		background: var(--color-bg);
		color: var(--color-primary);
		box-shadow: var(--shadow-sm);
	}
</style>
