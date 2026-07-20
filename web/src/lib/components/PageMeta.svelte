<script lang="ts">
	let {
		title,
		description,
		path,
		noindex = false,
		imageAlt = 'Conary package index: public package metadata and policy-gated conversion evidence.'
	}: {
		title: string;
		description: string;
		path?: string;
		noindex?: boolean;
		imageAlt?: string;
	} = $props();

	const siteUrl = 'https://remi.conary.io';
	const imageUrl = `${siteUrl}/brand/conary-social.png`;
	let canonicalUrl = $derived(path ? new URL(path, siteUrl).toString() : undefined);
</script>

<svelte:head>
	<title>{title}</title>
	<meta name="description" content={description} />
	{#if noindex}<meta name="robots" content="noindex" />{/if}
	{#if canonicalUrl}<link rel="canonical" href={canonicalUrl} />{/if}
	<meta property="og:site_name" content="Conary Package Index" />
	<meta property="og:type" content="website" />
	<meta property="og:title" content={title} />
	<meta property="og:description" content={description} />
	{#if canonicalUrl}<meta property="og:url" content={canonicalUrl} />{/if}
	<meta property="og:image" content={imageUrl} />
	<meta property="og:image:width" content="1200" />
	<meta property="og:image:height" content="630" />
	<meta property="og:image:alt" content={imageAlt} />
	<meta name="twitter:card" content="summary_large_image" />
	<meta name="twitter:title" content={title} />
	<meta name="twitter:description" content={description} />
	<meta name="twitter:image" content={imageUrl} />
	<meta name="twitter:image:alt" content={imageAlt} />
</svelte:head>
