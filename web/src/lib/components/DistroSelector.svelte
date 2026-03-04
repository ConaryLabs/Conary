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
		display: inline-flex;
		gap: 2px;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		padding: 3px;
	}

	button {
		padding: 0.4375rem 1.125rem;
		border: none;
		border-radius: calc(var(--radius-md) - 3px);
		background: none;
		color: var(--color-text-secondary);
		font-size: 0.8125rem;
		font-weight: 500;
		transition: all 0.15s;
	}

	button:hover {
		color: var(--color-text);
	}

	button.active {
		background: var(--color-accent-subtle);
		color: var(--color-accent);
	}
</style>
