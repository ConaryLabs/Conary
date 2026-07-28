<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';

	const routes = [
		{
			label: 'security',
			tone: 'urgent',
			title: 'Report a security issue',
			body: 'Use a private GitHub security advisory. Do not open a public issue for a vulnerability — a public issue discloses it the moment you file it.',
			href: 'https://github.com/ConaryLabs/Conary/security/advisories/new',
			action: 'Open a private advisory'
		},
		{
			label: 'bug',
			tone: 'normal',
			title: 'Report a bug',
			body: 'Include the command you ran, your distribution and architecture, the package name, the Conary version, the source package format, and the exact error. A failure that cannot be reproduced cannot be fixed.',
			href: 'https://github.com/ConaryLabs/Conary/issues/new/choose',
			action: 'Open an issue'
		},
		{
			label: 'feature',
			tone: 'normal',
			title: 'Request a capability',
			body: 'Describe the package or workflow that does not work today rather than the implementation you have in mind. A missing capability model is an engineering defect worth filing precisely.',
			href: 'https://github.com/ConaryLabs/Conary/issues/new/choose',
			action: 'Open an issue'
		},
		{
			label: 'question',
			tone: 'normal',
			title: 'Ask a question',
			body: 'Questions about how Conary behaves, what is supported today, or whether an approach makes sense belong in Discussions, where the answer stays searchable for the next person.',
			href: 'https://github.com/ConaryLabs/Conary/discussions',
			action: 'Open a discussion'
		}
	];
</script>

<PageMeta
	title="Contact — Conary"
	description="Where to send a Conary bug report, security advisory, question, or anything else."
	path="/contact/"
/>

<PageIntro
	eyebrow="Contact"
	title="Where to send it."
	description="Bugs, questions, and capability requests go to GitHub so the answers stay public and searchable. Security issues have a private path. Anything else reaches the maintainer directly."
/>

<section class="section contact-section">
	<div class="container">
		<ul class="route-list">
			{#each routes as route}
				<li class="route" class:urgent={route.tone === 'urgent'}>
					<span class="route-label">{route.label}</span>
					<div class="route-body">
						<h2>{route.title}</h2>
						<p>{route.body}</p>
					</div>
					<a class="btn btn-secondary route-action" href={route.href}>
						{route.action} <span aria-hidden="true">↗</span>
					</a>
				</li>
			{/each}
		</ul>

		<div class="direct">
			<div class="direct-copy">
				<p class="eyebrow">Everything else</p>
				<h2>Reach the maintainer</h2>
				<p>
					Conary is built by one person. Anything that does not fit above —
					packaging a project, running Conary somewhere unusual, press, or working
					together — goes straight to the inbox.
				</p>
				<p class="direct-availability">
					Available for remote engineering work, contract or employed: Rust systems,
					backend services, and Linux tooling. Open to relocating to Europe.
				</p>
			</div>
			<a class="btn btn-primary direct-mail" href="mailto:peter@conary.io">peter@conary.io</a>
		</div>
	</div>
</section>

<style>
	.contact-section {
		padding-top: clamp(2.5rem, 6vw, 4rem);
	}

	.route-list {
		margin: 0;
		padding: 0;
		list-style: none;
		border-top: 1px solid var(--color-border);
	}

	.route {
		display: grid;
		grid-template-columns: 7rem minmax(0, 1fr) auto;
		gap: clamp(1rem, 3vw, 2.5rem);
		align-items: start;
		padding-block: clamp(1.5rem, 3vw, 2.25rem);
		border-bottom: 1px solid var(--color-border);
	}

	.route-label {
		padding-top: 0.3rem;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: 0.7rem;
		letter-spacing: 0.1em;
		text-transform: uppercase;
	}

	/* The security row is the one where guessing wrong causes harm, so it is the
	   only row that carries the warning color. */
	.route.urgent .route-label {
		color: var(--color-orange);
	}

	.route.urgent .route-body {
		padding-left: 1rem;
		border-left: 3px solid var(--color-orange);
	}

	.route-body h2 {
		margin-bottom: 0.5rem;
		font-family: var(--font-body);
		font-size: var(--step-sub);
		font-weight: 600;
		letter-spacing: 0;
	}

	.route-body p {
		max-width: 62ch;
		margin: 0;
		color: var(--color-mist);
		font-size: 0.95rem;
		line-height: 1.7;
	}

	.route-action {
		flex: 0 0 auto;
		white-space: nowrap;
	}

	.direct {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: clamp(1.5rem, 4vw, 3rem);
		align-items: center;
		margin-top: clamp(2.5rem, 6vw, 4rem);
		padding: clamp(1.75rem, 4vw, 2.75rem);
		border: 1px solid var(--color-control-border);
		background: var(--color-layer);
	}

	.direct-copy h2 {
		margin-bottom: 0.75rem;
		font-size: var(--step-section);
		font-weight: 700;
	}

	.direct-copy p {
		max-width: 60ch;
		margin: 0;
		color: var(--color-mist);
	}

	.direct-availability {
		margin-top: 1rem !important;
		color: var(--color-muted) !important;
		font-size: 0.92rem;
	}

	.direct-mail {
		font-family: var(--font-mono);
		white-space: nowrap;
	}

	@media (max-width: 860px) {
		.route {
			grid-template-columns: 6rem minmax(0, 1fr);
		}

		.route-action {
			grid-column: 2;
			justify-self: start;
		}

		.direct {
			grid-template-columns: 1fr;
			align-items: stretch;
		}
	}

	@media (max-width: 520px) {
		.route {
			grid-template-columns: 1fr;
			gap: 0.75rem;
		}

		.route-action,
		.route-body {
			grid-column: 1;
		}

		.route.urgent .route-body {
			padding-left: 0.85rem;
		}
	}
</style>
