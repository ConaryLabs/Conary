<svelte:head>
	<title>About - Conary</title>
	<meta name="description" content="About the Conary project -- goals, architecture, and how to contribute." />
</svelte:head>

<section class="page-hero">
	<div class="container">
		<h1 class="page-title animate-in" style="--stagger: 0">About Conary</h1>
		<p class="page-desc animate-in" style="--stagger: 1">
			Reviving a visionary 2005 design -- now with immutable system generations.
		</p>
	</div>
</section>

<section class="about-section">
	<div class="container about-content">
		<div class="about-block animate-in" style="--stagger: 2">
			<h2>Origins</h2>
			<p>
				Conary takes its name and core philosophy from the
				<a href="https://en.wikipedia.org/wiki/Conary_(package_manager)" target="_blank" rel="noopener noreferrer">original Conary package manager</a>
				developed by the team at <strong>rPath</strong> in the mid-2000s, many of them former Red Hat engineers.
				That project pioneered ideas that were years ahead of
				the mainstream: content-addressable storage for packages, repository-level binary diffs,
				a SAT-based dependency resolver, and atomic rollback of system state.
			</p>
			<p>
				rPath's Conary proved these concepts worked in production, but it was written in Python 2
				and tied to rPath's own Linux distribution (rPath Linux / Foresight Linux). When rPath was
				acquired by SAS in 2011, the project went dormant. The ideas, however, remained sound.
			</p>
			<p>
				This project is a ground-up reimplementation in Rust -- not a fork or a port. The original
				source is long gone from active development, but the design principles endure: treat the
				filesystem as a content store, make every operation atomic and reversible, and resolve
				dependencies correctly the first time. We carry the name forward as a tribute to the
				engineering that got there first.
			</p>
		</div>

		<div class="about-block animate-in" style="--stagger: 3">
			<h2>The Problem</h2>
			<p>
				Linux package management is fragmented. Every distribution maintains its own
				package format, its own repositories, its own tools. Switching distros means learning
				new commands, losing muscle memory, and accepting that software availability varies
				wildly. Even within a single distro, package managers haven't fundamentally changed
				in decades -- most still lack atomic transactions, content-addressable storage, or
				efficient binary updates.
			</p>
		</div>

		<div class="about-block animate-in" style="--stagger: 5">
			<h2>The Approach</h2>
			<p>
				Conary doesn't ask upstream maintainers to change anything. It works with existing
				RPM, DEB, and Arch packages through <strong>Remi</strong>, a conversion proxy that
				transparently converts upstream packages into Conary's native CCS format. This means
				immediate access to tens of thousands of packages across three major distributions,
				with no upstream changes required.
			</p>
			<p>
				Under the hood, every install, remove, or upgrade builds a new EROFS image and
				atomically switches the composefs mount. Content-addressable storage (SHA-256 + XXH128)
				handles file-level deduplication, a SAT-based dependency resolver (via resolvo) solves
				dependencies, and EROFS binary deltas keep updates small. The kernel enforces integrity
				via fs-verity on every file read -- tampered files cause I/O errors, not silent corruption.
			</p>
		</div>

		<div class="about-block animate-in" style="--stagger: 6">
			<h2>Architecture</h2>
			<div class="arch-grid">
				<div class="arch-item">
					<h3>CAS Layer</h3>
					<p>Content-addressable storage. Files stored by hash, not by package. Identical files are automatically deduplicated.</p>
				</div>
				<div class="arch-item">
					<h3>Composefs Transactions</h3>
					<p>Every operation builds a new EROFS image and switches the composefs mount. Previous generation remains intact for instant rollback.</p>
				</div>
				<div class="arch-item">
					<h3>Resolver</h3>
					<p>SAT-based dependency resolution via resolvo. Handles conflicts, virtual provides, and cross-distro deps.</p>
				</div>
				<div class="arch-item">
					<h3>Format Parsers</h3>
					<p>Native parsers for RPM, DEB, and Arch packages. Unified metadata model across all formats.</p>
				</div>
				<div class="arch-item">
					<h3>Delta Engine</h3>
					<p>On-demand binary deltas between any two versions. Zstd dictionary compression. 60-90% smaller updates.</p>
				</div>
				<div class="arch-item">
					<h3>System Model</h3>
					<p>Declarative system configuration. Define desired state, Conary computes and applies the diff.</p>
				</div>
				<div class="arch-item">
					<h3>Generations</h3>
					<p>Immutable EROFS filesystem images with composefs overlay. Live-switch between verified system states.</p>
				</div>
				<div class="arch-item">
					<h3>Bootstrap</h3>
					<p>Staged pipeline to build a complete system from scratch. Cross-compiler through bootable image.</p>
				</div>
			</div>
		</div>

		<div class="about-block animate-in" style="--stagger: 7">
			<h2>Tech Stack</h2>
			<div class="tech-list">
				<div class="tech-item">
					<span class="tech-label">Language</span>
					<span class="tech-value">Rust (Edition 2024), 6-member Cargo workspace</span>
				</div>
				<div class="tech-item">
					<span class="tech-label">Filesystem</span>
					<span class="tech-value">EROFS + composefs (composefs-rs), fs-verity</span>
				</div>
				<div class="tech-item">
					<span class="tech-label">Database</span>
					<span class="tech-value">SQLite (schema version 65, DB-first runtime state)</span>
				</div>
				<div class="tech-item">
					<span class="tech-label">Hashing</span>
					<span class="tech-value">SHA-256, XXH128</span>
				</div>
				<div class="tech-item">
					<span class="tech-label">Compression</span>
					<span class="tech-value">Zstd, Gzip, XZ</span>
				</div>
				<div class="tech-item">
					<span class="tech-label">Server</span>
					<span class="tech-value">Axum + Tantivy (full-text search)</span>
				</div>
				<div class="tech-item">
					<span class="tech-label">Resolver</span>
					<span class="tech-value">resolvo (SAT solver)</span>
				</div>
			</div>
		</div>

		<div class="about-block animate-in" style="--stagger: 8">
			<h2>Contributing</h2>
			<p>
				Conary is open source and welcomes contributions. The codebase is well-structured
				with thousands of tests across unit, integration, and harness coverage, comprehensive CI (clippy, test,
				and release workflows), and good-first-issue labels for newcomers.
			</p>
			<div class="about-links">
				<a href="https://github.com/ConaryLabs/Conary" target="_blank" rel="noopener noreferrer" class="btn btn-outline">GitHub</a>
				<a href="https://github.com/ConaryLabs/Conary/blob/main/CONTRIBUTING.md" target="_blank" rel="noopener noreferrer" class="btn btn-outline">Contributing Guide</a>
				<a href="https://github.com/ConaryLabs/Conary/discussions" target="_blank" rel="noopener noreferrer" class="btn btn-outline">Discussions</a>
				<a href="https://github.com/ConaryLabs/Conary/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22" target="_blank" rel="noopener noreferrer" class="btn btn-outline">Good First Issues</a>
			</div>
		</div>
	</div>
</section>

<style>
	.page-hero {
		padding: 4rem 0 2rem;
		text-align: center;
	}

	.page-title {
		font-family: var(--font-display);
		font-size: 2.75rem;
		font-weight: 800;
		letter-spacing: -0.04em;
		margin-bottom: 0.5rem;
	}

	.page-desc {
		font-size: 1.0625rem;
		color: var(--color-text-secondary);
		font-weight: 300;
	}

	.about-section {
		padding: 2rem 0 5rem;
	}

	.about-content {
		max-width: 740px;
		margin: 0 auto;
	}

	.about-block {
		margin-bottom: 3rem;
	}

	.about-block h2 {
		font-family: var(--font-display);
		font-size: 1.25rem;
		font-weight: 700;
		color: var(--color-accent);
		margin-bottom: 0.75rem;
	}

	.about-block p {
		font-size: 0.9375rem;
		color: var(--color-text-secondary);
		line-height: 1.7;
		margin: 0 0 0.75rem;
		font-weight: 300;
	}

	.about-block p strong {
		color: var(--color-text);
		font-weight: 500;
	}

	.about-block p a {
		color: var(--color-accent);
		text-decoration: none;
		border-bottom: 1px solid transparent;
		transition: border-color 0.15s;
	}

	.about-block p a:hover {
		border-bottom-color: var(--color-accent);
	}

	.arch-grid {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: 0.875rem;
	}

	.arch-item {
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		padding: 1.25rem;
	}

	.arch-item h3 {
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-accent);
		margin-bottom: 0.375rem;
	}

	.arch-item p {
		font-size: 0.8125rem;
		line-height: 1.5;
		margin: 0;
	}

	.tech-list {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.tech-item {
		display: flex;
		align-items: center;
		padding: 0.625rem 1rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
	}

	.tech-label {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-text-muted);
		min-width: 120px;
		font-weight: 500;
	}

	.tech-value {
		font-size: 0.875rem;
		color: var(--color-text);
	}

	.about-links {
		display: flex;
		gap: 0.75rem;
		margin-top: 1.25rem;
	}

	.btn {
		display: inline-flex;
		align-items: center;
		padding: 0.625rem 1.25rem;
		border-radius: var(--radius-md);
		font-size: 0.875rem;
		font-weight: 600;
		text-decoration: none;
		transition: all 0.15s;
	}

	.btn-outline {
		background: transparent;
		color: var(--color-text);
		border: 1px solid var(--color-border-hover);
	}

	.btn-outline:hover {
		border-color: var(--color-accent);
		color: var(--color-accent);
		transform: translateY(-1px);
	}

	@media (max-width: 768px) {
		.page-title { font-size: 2rem; }
		.arch-grid { grid-template-columns: 1fr; }
		.tech-label { min-width: 90px; }
	}
</style>
