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

<div class="distro-selector" aria-label="Distribution filter">
	{#each distros as distro}
		<button
			type="button"
			aria-pressed={selected === distro.id}
			class:active={selected === distro.id}
			onclick={() => onselect(distro.id)}
		>
			{distro.label}
		</button>
	{/each}
</div>

<style>
	.distro-selector {
		display: inline-flex;
		max-width: 100%;
		padding: 3px;
		overflow-x: auto;
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-sm);
		background: var(--color-code-bg);
	}

	button {
		min-height: 40px;
		padding: 0.45rem 1rem;
		border: 0;
		border-radius: 2px;
		color: var(--color-mist);
		background: transparent;
		font-family: var(--font-mono);
		font-size: 0.72rem;
		font-weight: 500;
		transition: color 150ms ease, background 150ms ease;
	}

	button:hover {
		color: var(--color-ivory);
	}

	button.active {
		color: var(--color-field);
		background: var(--color-cyan);
	}
</style>
