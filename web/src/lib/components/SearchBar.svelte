<script lang="ts">
	import { goto } from '$app/navigation';
	import { suggestPackages } from '$lib/api';

	let {
		value = '',
		placeholder = 'Search packages…'
	}: {
		value?: string;
		placeholder?: string;
	} = $props();

	let query = $state('');
	let suggestions: string[] = $state([]);
	let showSuggestions = $state(false);
	let selectedIndex = $state(-1);
	let debounceTimer: ReturnType<typeof setTimeout> | undefined;

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

	function selectSuggestion(suggestion: string) {
		query = suggestion;
		showSuggestions = false;
		submit();
	}

	function handleKeydown(event: KeyboardEvent) {
		if (!showSuggestions) {
			if (event.key === 'Enter') submit();
			return;
		}

		switch (event.key) {
			case 'ArrowDown':
				event.preventDefault();
				selectedIndex = Math.min(selectedIndex + 1, suggestions.length - 1);
				break;
			case 'ArrowUp':
				event.preventDefault();
				selectedIndex = Math.max(selectedIndex - 1, -1);
				break;
			case 'Enter':
				event.preventDefault();
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
		<span class="search-prompt" aria-hidden="true">find /</span>
		<input
			bind:value={query}
			oninput={handleInput}
			onkeydown={handleKeydown}
			onfocus={() => { if (suggestions.length > 0) showSuggestions = true; }}
			onblur={handleBlur}
			type="search"
			{placeholder}
			autocomplete="off"
			spellcheck="false"
			role="combobox"
			aria-label="Search packages"
			aria-expanded={showSuggestions}
			aria-autocomplete="list"
			aria-controls="search-suggestions"
			aria-activedescendant={selectedIndex >= 0 ? `search-suggestion-${selectedIndex}` : undefined}
		/>
		<button class="search-submit" onclick={submit} aria-label="Submit package search">
			Search <span aria-hidden="true">→</span>
		</button>
	</div>

	{#if showSuggestions}
		<ul id="search-suggestions" class="suggestions" role="listbox">
			{#each suggestions as suggestion, i}
				<li id="search-suggestion-{i}" role="option" aria-selected={i === selectedIndex} class:selected={i === selectedIndex}>
					<button tabindex="-1" onclick={() => selectSuggestion(suggestion)}>{suggestion}</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.search-bar {
		position: relative;
		width: 100%;
		max-width: 680px;
	}

	.search-input-wrapper {
		display: flex;
		align-items: stretch;
		min-height: 54px;
		border: 1px solid var(--color-control-border);
		border-radius: var(--radius-sm);
		background: var(--color-code-bg);
		transition: border-color 150ms ease;
	}

	.search-input-wrapper:focus-within {
		border-color: var(--color-cyan);
	}

	.search-input-wrapper:has(input:focus-visible) {
		outline: 3px solid var(--color-orange);
		outline-offset: 4px;
	}

	.search-prompt {
		display: flex;
		align-items: center;
		margin-left: 1rem;
		color: var(--color-orange);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		font-weight: 500;
		user-select: none;
	}

	input {
		min-width: 0;
		flex: 1;
		padding: 0.85rem 0.8rem;
		border: 0;
		outline: 0;
		color: var(--color-ivory);
		background: transparent;
		font-size: 0.98rem;
	}

	input::placeholder {
		color: var(--color-muted);
	}

	.search-submit {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.8rem 1.1rem;
		border: 0;
		border-left: 1px solid var(--color-control-border);
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
		color: var(--color-field);
		background: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.76rem;
		font-weight: 500;
	}

	.search-submit:hover {
		background: var(--color-cyan-bright);
	}

	.suggestions {
		position: absolute;
		top: calc(100% + 0.35rem);
		left: 0;
		right: 0;
		z-index: 100;
		max-height: 300px;
		margin: 0;
		padding: 0.35rem 0;
		overflow-y: auto;
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-sm);
		background: var(--color-layer);
		box-shadow: var(--shadow-lg);
		list-style: none;
	}

	.suggestions li button {
		display: block;
		width: 100%;
		padding: 0.55rem 1rem;
		border: 0;
		color: var(--color-ivory);
		background: transparent;
		font-family: var(--font-mono);
		font-size: 0.85rem;
		text-align: left;
	}

	.suggestions li button:hover,
	.suggestions li.selected button {
		color: var(--color-cyan);
		background: var(--color-accent-subtle);
	}

	@media (max-width: 520px) {
		.search-prompt {
			display: none;
		}

		.search-submit {
			padding-inline: 0.85rem;
		}
	}
</style>
