<script lang="ts">
	import ErrorPanel from '$lib/components/ErrorPanel.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
</script>

<PageMeta
	title="Page not found — Conary"
	description="The requested Conary page could not be found."
	noindex
/>

<ErrorPanel
	status={404}
	eyebrow="route missing"
	title="This path is outside the current map."
	description="The page you requested is not part of the Conary site. Return to the evaluation path or open the install guide directly."
/>
