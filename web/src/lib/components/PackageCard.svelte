<script lang="ts">
	let {
		name,
		distro,
		version = '',
		description = '',
		downloads = 0,
		converted = false,
		size = 0
	}: {
		name: string;
		distro: string;
		version?: string;
		description?: string;
		downloads?: number;
		converted?: boolean;
		size?: number;
	} = $props();

	function formatSize(bytes: number): string {
		if (bytes === 0) return '';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	function formatDownloads(count: number): string {
		if (count === 0) return '';
		if (count < 1000) return String(count);
		if (count < 1_000_000) return `${(count / 1000).toFixed(1)}k`;
		return `${(count / 1_000_000).toFixed(1)}M`;
	}
</script>

<a href="/packages/{distro}/{name}" class="package-card">
	<div class="card-header">
		<span class="card-name">{name}</span>
		{#if version}
			<span class="card-version">{version}</span>
		{/if}
	</div>
	{#if description}
		<p class="card-description">{description}</p>
	{/if}
	<div class="card-meta">
		<span class="card-distro">{distro}</span>
		{#if converted}
			<span class="card-badge badge-converted">CCS</span>
		{/if}
		{#if size > 0}
			<span class="card-size">{formatSize(size)}</span>
		{/if}
		{#if downloads > 0}
			<span class="card-downloads">{formatDownloads(downloads)} downloads</span>
		{/if}
	</div>
</a>

<style>
	.package-card {
		display: block;
		padding: 1rem 1.25rem;
		background: var(--color-card-bg);
		border: 1px solid var(--color-card-border);
		border-radius: var(--radius-md);
		text-decoration: none;
		color: var(--color-text);
		transition: border-color 0.15s, box-shadow 0.15s;
	}

	.package-card:hover {
		border-color: var(--color-primary);
		box-shadow: var(--shadow-md);
		text-decoration: none;
	}

	.card-header {
		display: flex;
		align-items: baseline;
		gap: 0.5rem;
		margin-bottom: 0.375rem;
	}

	.card-name {
		font-weight: 600;
		font-size: 1.0625rem;
		color: var(--color-primary);
	}

	.card-version {
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.card-description {
		margin: 0 0 0.625rem;
		font-size: 0.875rem;
		color: var(--color-text-secondary);
		line-height: 1.5;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.card-meta {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.card-distro {
		font-weight: 500;
		text-transform: capitalize;
	}

	.card-badge {
		padding: 0.1em 0.5em;
		border-radius: var(--radius-sm);
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.03em;
	}

	.badge-converted {
		background: var(--color-success);
		color: #fff;
	}

	.card-size,
	.card-downloads {
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}
</style>
