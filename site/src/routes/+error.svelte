<script lang="ts">
	import { page } from '$app/state';
	import ErrorPanel from '$lib/components/ErrorPanel.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';

	let isNotFound = $derived(page.status === 404);
	let title = $derived(isNotFound ? 'Page not found — Conary' : `Request failed (${page.status}) — Conary`);
	let description = $derived(
		isNotFound
			? 'The requested Conary page could not be found.'
			: 'Conary could not complete this request. Return home and try the documented path again.'
	);
</script>

<PageMeta {title} {description} noindex />

<ErrorPanel
	status={page.status}
	eyebrow={isNotFound ? 'route missing' : 'request failed'}
	title={isNotFound ? 'This path is outside the current map.' : 'This request left the supported path.'}
	description={isNotFound
		? 'The page you requested is not part of the Conary site. Return to the evaluation path or open the install guide directly.'
		: 'The site could not complete this request. Return home, then retry from a documented page.'}
/>
