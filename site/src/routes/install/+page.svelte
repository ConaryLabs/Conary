<svelte:head>
	<title>Install - Conary</title>
	<meta name="description" content="Try the Conary limited preview on Fedora 44, Ubuntu 26.04 LTS, or Arch Linux with reversible native-package adoption." />
</svelte:head>

<section class="page-hero">
	<div class="container">
		<h1 class="page-title animate-in" style="--stagger: 0">Install Conary</h1>
		<p class="page-desc animate-in" style="--stagger: 1">
			Start with a reversible adoption preview on a VM or non-critical host.
		</p>
	</div>
</section>

<section class="install-section">
	<div class="container">
		<!-- Five-minute preview -->
		<div class="install-block animate-in" style="--stagger: 2">
			<h2>Five-Minute Preview</h2>
			<p class="install-note">
				The limited preview path is adoption-led: Conary observes native packages while
				dnf, apt, or pacman remains authoritative. First Remi-backed package use may be
				slower while RPM/DEB/Arch metadata is converted into CCS.
			</p>
			<div class="terminal">
				<div class="terminal-header">
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-title">terminal</span>
				</div>
				<div class="terminal-body">
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary system init</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary repo add remi https://remi.conary.io</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary repo sync</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary system adopt --system --dry-run</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary system adopt --status</span>
					</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary --allow-live-system-mutation system adopt --system</span>
					</div>
					<div class="terminal-line t-output">The long flag marks the point where Conary changes the active host.</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary system unadopt --all --dry-run</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary --allow-live-system-mutation system unadopt --all</span>
					</div>
				</div>
			</div>
			<p class="install-note">
				Before selecting a Conary generation, unadopt removes Conary tracking without
				deleting native package files.
			</p>
		</div>

		<!-- Build from source -->
		<div class="install-block animate-in" style="--stagger: 5">
			<h2>Developer Build</h2>
			<p class="install-note">
				Release binaries are not linked for this preview tag yet, so source builds are
				the current developer path.
				Requires Rust 1.96+, SQLite development headers, and Linux 6.2+ with
				composefs and EROFS support. Conary uses Linux-specific kernel APIs
				(composefs, fs-verity, namespaces, landlock, seccomp) and does not
				currently build on macOS or Windows.
			</p>
			<div class="terminal">
				<div class="terminal-header">
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-title">terminal</span>
				</div>
				<div class="terminal-body">
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">git clone https://github.com/ConaryLabs/Conary.git</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">cd Conary</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">cargo build --release</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">sudo install -m 755 target/release/conary /usr/local/bin/</span>
					</div>
				</div>
			</div>
		</div>

		<!-- First steps -->
		<div class="install-block animate-in" style="--stagger: 9">
			<h2>Conary-Owned Package Check</h2>
			<p class="install-note">After the adoption preview, try a Conary-owned dry-run before applying an install.</p>
			<div class="terminal">
				<div class="terminal-header">
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-dot" aria-hidden="true"></span>
					<span class="terminal-title">terminal</span>
				</div>
				<div class="terminal-body">
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary --version</span>
					</div>
					<div class="terminal-line t-output">conary 0.8.0</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary repo add remi https://remi.conary.io</span>
					</div>
					<div class="terminal-line t-output t-success">Repository added.</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary repo sync</span>
					</div>
					<div class="terminal-line t-output">Syncing metadata from 1 repository...</div>
					<div class="terminal-line t-output t-success">Sync complete. Tens of thousands of packages available.</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary install htop --dry-run</span>
					</div>
					<div class="terminal-line t-output">Resolving dependencies...</div>
					<div class="terminal-line t-output t-success">No host files changed.</div>
				</div>
			</div>
		</div>
		<!-- Distribution packages -->
		<div class="install-block animate-in" style="--stagger: 11">
			<h2>Distribution Packages</h2>
			<p class="install-note">
				The preview release matrix will link native RPM, DEB, and Arch artifacts
				when they are published. Until that artifact/provenance matrix is ready,
				use the developer build above.
			</p>
			<div class="distro-cards">
				<div class="distro-card distro-fedora">
					<div class="distro-badge">Fedora</div>
					<p class="distro-note">Fedora 44 / RPM-based</p>
				</div>
				<div class="distro-card distro-arch">
					<div class="distro-badge">Arch Linux</div>
					<p class="distro-note">PKGBUILD package</p>
				</div>
				<div class="distro-card distro-ubuntu">
					<div class="distro-badge">Ubuntu</div>
					<p class="distro-note">Ubuntu 26.04 LTS / DEB-based</p>
				</div>
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

	.install-section {
		padding: 2rem 0 5rem;
	}

	.install-block {
		max-width: 740px;
		margin: 0 auto 3.5rem;
	}

	.install-block h2 {
		font-family: var(--font-display);
		font-size: 1.25rem;
		font-weight: 700;
		margin-bottom: 0.5rem;
		color: var(--color-accent);
	}

	.install-note {
		font-size: 0.9375rem;
		color: var(--color-text-secondary);
		margin-bottom: 1.25rem;
		font-weight: 300;
	}

	.terminal {
		background: var(--color-code-bg);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		overflow: hidden;
		box-shadow: var(--shadow-lg);
	}

	.terminal-header {
		display: flex;
		align-items: center;
		gap: 0.375rem;
		padding: 0.75rem 1rem;
		background: var(--color-surface);
		border-bottom: 1px solid var(--color-border);
	}

	.terminal-dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--color-text-muted);
		opacity: 0.4;
	}

	.terminal-title {
		font-family: var(--font-mono);
		font-size: 0.6875rem;
		color: var(--color-text-muted);
		margin-left: 0.5rem;
	}

	.terminal-body {
		padding: 1.25rem 1.5rem;
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		line-height: 1.8;
	}

	.terminal-line { white-space: nowrap; overflow: hidden; }
	.t-prompt { color: var(--color-accent); font-weight: 500; margin-right: 0.625rem; user-select: none; }
	.t-cmd { color: var(--color-text); }
	.t-output { color: var(--color-text-secondary); padding-left: 1.375rem; }
	.t-success { color: var(--color-success); }
	.t-blank { height: 0.5rem; }

	.distro-cards {
		display: grid;
		grid-template-columns: 1fr;
		gap: 1.25rem;
	}

	.distro-card {
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		padding: 1.5rem;
		border-left: 3px solid;
	}

	.distro-fedora { border-left-color: var(--color-fedora); }
	.distro-arch { border-left-color: var(--color-arch); }
	.distro-ubuntu { border-left-color: var(--color-ubuntu); }

	.distro-badge {
		font-family: var(--font-display);
		font-size: 0.9375rem;
		font-weight: 700;
		margin-bottom: 0.875rem;
	}

	.distro-fedora .distro-badge { color: var(--color-fedora); }
	.distro-arch .distro-badge { color: var(--color-arch); }
	.distro-ubuntu .distro-badge { color: var(--color-ubuntu); }

	.distro-note {
		font-size: 0.75rem;
		color: var(--color-text-muted);
		margin-top: 0.75rem;
		margin-bottom: 0;
	}

	@media (max-width: 768px) {
		.page-title { font-size: 2rem; }
		.terminal-body { font-size: 0.6875rem; overflow-x: auto; }
	}
</style>
