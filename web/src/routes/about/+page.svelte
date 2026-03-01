<svelte:head>
	<title>About - Conary Package Index</title>
	<meta name="description" content="Learn about Conary, the cross-distribution package manager, and Remi, the on-demand conversion proxy." />
</svelte:head>

<div class="container page">
	<div class="content">
		<h1>About Conary</h1>

		<p>
			Conary is a cross-distribution Linux package manager that unifies package management
			across Fedora, Arch Linux, Ubuntu, and more. It introduces the CCS (Conary Container
			Store) format for enhanced package metadata, content-addressable storage, and
			reproducible installations.
		</p>

		<h2>What is Remi?</h2>

		<p>
			Remi is Conary's on-demand conversion proxy. It sits between your system and upstream
			distribution repositories, transparently converting legacy packages (RPM, DEB, Arch)
			into the CCS format. This gives you the benefits of CCS -- content-addressable storage,
			binary deltas, hermetic builds -- without requiring upstream distributions to change
			anything.
		</p>

		<h2>Key Features</h2>

		<ul>
			<li>
				<strong>Cross-distribution</strong> -- Manage packages from Fedora, Arch, and
				Ubuntu with a single tool
			</li>
			<li>
				<strong>Content-addressable storage</strong> -- Deduplicate files across packages
				using CAS
			</li>
			<li>
				<strong>Binary deltas</strong> -- Download only what changed between package
				versions
			</li>
			<li>
				<strong>Atomic transactions</strong> -- Install, remove, and rollback operations
				are crash-safe
			</li>
			<li>
				<strong>Hermetic builds</strong> -- Reproducible package builds with network
				isolation
			</li>
			<li>
				<strong>CAS federation</strong> -- Peer-to-peer chunk sharing for bandwidth
				savings
			</li>
			<li>
				<strong>System model</strong> -- Declarative OS state for reproducible systems
			</li>
		</ul>

		<h2>Getting Started</h2>

		<p>Install a package with Conary:</p>

		<pre><code>conary install nginx</code></pre>

		<p>Search for packages:</p>

		<pre><code>conary search "web server"</code></pre>

		<p>View package details:</p>

		<pre><code>conary info nginx</code></pre>

		<h2>Package Index</h2>

		<p>
			This web interface provides a searchable index of all packages available through Remi.
			You can browse packages by distribution, search by name or description, and view
			detailed information including versions, dependencies, and download statistics.
		</p>

		<h2>Links</h2>

		<ul>
			<li><a href="/packages">Browse packages</a></li>
			<li><a href="/stats">View statistics</a></li>
		</ul>
	</div>
</div>

<style>
	.page {
		padding: 2rem 1.5rem;
	}

	.content {
		max-width: 720px;
	}

	h1 {
		font-size: 2rem;
		margin-bottom: 1rem;
	}

	h2 {
		font-size: 1.375rem;
		margin-top: 2.5rem;
		margin-bottom: 0.75rem;
	}

	p {
		line-height: 1.7;
		color: var(--color-text);
		margin-bottom: 1rem;
	}

	ul {
		line-height: 1.8;
		padding-left: 1.5rem;
		margin-bottom: 1rem;
	}

	li {
		margin-bottom: 0.375rem;
	}

	pre {
		margin-bottom: 1rem;
	}
</style>
