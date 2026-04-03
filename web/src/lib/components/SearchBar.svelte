<script lang="ts">
	import { goto } from '$app/navigation';
	import { suggestPackages } from '$lib/api';

	let {
		value = '',
		placeholder = 'Search packages...',
		autofocus = false
	}: {
		value?: string;
		placeholder?: string;
		autofocus?: boolean;
	} = $props();

	let query = $state('');
	let suggestions: string[] = $state([]);
	let showSuggestions = $state(false);
	let selectedIndex = $state(-1);
	let debounceTimer: ReturnType<typeof setTimeout> | undefined;
	let inputEl: HTMLInputElement | undefined = $state();

	$effect(() => {
		query = value;
	});

	function debounce(fn: () => void, ms: number) {
		clearTimeout(debounceTimer);
		debounceTimer = setTimeout(fn, ms);
	}

	function handleInput() {
		selectedIndex = -1;
		if (query.length >= 2) {
			debounce(async () => {
				try {
					const resp = await suggestPackages(query);
					suggestions = resp.suggestions ?? [];
					showSuggestions = suggestions.length > 0;
				} catch {
					suggestions = [];
					showSuggestions = false;
				}
			}, 300);
		} else {
			suggestions = [];
			showSuggestions = false;
		}
	}

	function submit() {
		const q = query.trim();
		if (!q) return;
		showSuggestions = false;
		goto(`/search?q=${encodeURIComponent(q)}`);
	}

	function selectSuggestion(s: string) {
		query = s;
		showSuggestions = false;
		submit();
	}

	function handleKeydown(e: KeyboardEvent) {
		if (!showSuggestions) {
			if (e.key === 'Enter') submit();
			return;
		}

		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				selectedIndex = Math.min(selectedIndex + 1, suggestions.length - 1);
				break;
			case 'ArrowUp':
				e.preventDefault();
				selectedIndex = Math.max(selectedIndex - 1, -1);
				break;
			case 'Enter':
				e.preventDefault();
				if (selectedIndex >= 0 && selectedIndex < suggestions.length) {
					selectSuggestion(suggestions[selectedIndex]);
				} else {
					submit();
				}
				break;
			case 'Escape':
				showSuggestions = false;
				selectedIndex = -1;
				break;
		}
	}

	function handleBlur() {
		setTimeout(() => {
			showSuggestions = false;
		}, 200);
	}
</script>

<div class="search-bar">
	<div class="search-input-wrapper">
		<span class="search-prompt" aria-hidden="true">&gt;_</span>
		<input
			bind:this={inputEl}
			bind:value={query}
			oninput={handleInput}
			onkeydown={handleKeydown}
			onfocus={() => { if (suggestions.length > 0) showSuggestions = true; }}
			onblur={handleBlur}
			type="text"
			{placeholder}
			autocomplete="off"
			spellcheck="false"
			role="combobox"
			aria-expanded={showSuggestions}
			aria-autocomplete="list"
			aria-controls="search-suggestions"
		/>
		<button class="search-submit" onclick={submit} aria-label="Search">
			<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
				<path fill-rule="evenodd" d="M10.293 3.293a1 1 0 011.414 0l6 6a1 1 0 010 1.414l-6 6a1 1 0 01-1.414-1.414L14.586 11H3a1 1 0 110-2h11.586l-4.293-4.293a1 1 0 010-1.414z" clip-rule="evenodd" />
			</svg>
		</button>
	</div>

	{#if showSuggestions}
		<ul id="search-suggestions" class="suggestions" role="listbox">
			{#each suggestions as suggestion, i}
				<li
					role="option"
					aria-selected={i === selectedIndex}
					class:selected={i === selectedIndex}
				>
					<button onclick={() => selectSuggestion(suggestion)}>
						{suggestion}
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.search-bar {
		position: relative;
		width: 100%;
		max-width: 640px;
	}

	.search-input-wrapper {
		display: flex;
		align-items: center;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		transition: border-color 0.2s, box-shadow 0.2s;
	}

	.search-input-wrapper:focus-within {
		border-color: var(--color-accent);
		box-shadow: 0 0 0 3px var(--color-accent-subtle), var(--shadow-glow);
	}

	.search-prompt {
		font-family: var(--font-mono);
		font-size: 0.9375rem;
		font-weight: 500;
		color: var(--color-accent);
		margin-left: 1.125rem;
		flex-shrink: 0;
		user-select: none;
	}

	input {
		flex: 1;
		border: none;
		background: none;
		padding: 0.875rem 0.75rem;
		font-size: 1rem;
		color: var(--color-text);
		outline: none;
		min-width: 0;
	}

	input::placeholder {
		color: var(--color-text-muted);
	}

	.search-submit {
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-accent);
		color: var(--color-bg);
		border: none;
		border-radius: 0 calc(var(--radius-lg) - 1px) calc(var(--radius-lg) - 1px) 0;
		padding: 0.875rem 1.125rem;
		transition: background-color 0.15s;
	}

	.search-submit:hover {
		background: var(--color-accent-hover);
	}

	.search-submit svg {
		width: 1.125rem;
		height: 1.125rem;
	}

	.suggestions {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		margin: 0;
		padding: 0.25rem 0;
		list-style: none;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-lg);
		z-index: 100;
		max-height: 300px;
		overflow-y: auto;
	}

	.suggestions li button {
		display: block;
		width: 100%;
		text-align: left;
		padding: 0.5rem 1rem 0.5rem 2.75rem;
		border: none;
		background: none;
		color: var(--color-text);
		font-size: 0.9375rem;
		font-family: var(--font-mono);
		transition: background 0.1s, color 0.1s;
	}

	.suggestions li button:hover,
	.suggestions li.selected button {
		background: var(--color-accent-subtle);
		color: var(--color-accent);
	}
</style>
