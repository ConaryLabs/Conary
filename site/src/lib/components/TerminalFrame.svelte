<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title = 'terminal',
		children
	}: {
		title?: string;
		children: Snippet;
	} = $props();
</script>

<div class="terminal-frame" role="group" aria-label={title}>
	<div class="terminal-header" aria-hidden="true">
		<span class="terminal-node orange"></span>
		<span class="terminal-node cyan"></span>
		<span class="terminal-node ivory"></span>
		<span class="terminal-title">{title}</span>
	</div>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal terminal content needs keyboard scrolling) -->
	<pre class="terminal-body scroll-region" tabindex="0" role="region" aria-label={`${title} commands`}><code>{@render children()}</code></pre>
</div>

<style>
	.terminal-frame {
		min-width: 0;
		overflow: hidden;
		border: 1px solid var(--color-control-border);
		border-radius: var(--radius-lg);
		background: var(--color-code-bg);
	}

	.terminal-header {
		display: flex;
		align-items: center;
		gap: 0.48rem;
		min-height: 42px;
		padding: 0.65rem 0.9rem;
		border-bottom: 1px solid var(--color-border);
		background: var(--color-layer);
	}

	.terminal-node {
		width: 0.55rem;
		height: 0.55rem;
		border-radius: 50%;
	}

	.terminal-node.orange {
		background: var(--color-orange);
	}

	.terminal-node.cyan {
		background: var(--color-cyan);
	}

	.terminal-node.ivory {
		background: var(--color-ivory);
	}

	.terminal-title {
		margin-left: 0.35rem;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: 0.68rem;
		letter-spacing: 0.06em;
	}

	.terminal-body {
		margin: 0;
		padding: clamp(1rem, 3vw, 1.5rem);
		overflow-x: auto;
		color: var(--color-ivory);
		background: transparent;
		font-size: clamp(0.72rem, 1.5vw, 0.84rem);
		line-height: 1.85;
		tab-size: 2;
	}

	.terminal-body:focus-visible {
		outline-offset: -4px;
	}

	.terminal-body code {
		font: inherit;
	}
</style>
