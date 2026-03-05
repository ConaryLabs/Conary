<svelte:head>
	<title>Install - Conary</title>
	<meta name="description" content="Install Conary on Fedora, Arch Linux, or Ubuntu in under a minute." />
</svelte:head>

<section class="page-hero">
	<div class="container">
		<h1 class="page-title animate-in" style="--stagger: 0">Install Conary</h1>
		<p class="page-desc animate-in" style="--stagger: 1">
			Get up and running on your distribution in under a minute.
		</p>
	</div>
</section>

<section class="install-section">
	<div class="container">
		<!-- Quick install -->
		<div class="install-block animate-in" style="--stagger: 3">
			<h2>Quick Install</h2>
			<p class="install-note">
				The recommended way to install Conary. Works on all supported distributions.
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
						<span class="t-cmd">curl -fsSL https://conary.io/install.sh | sh</span>
					</div>
				</div>
			</div>
			<p class="install-hint">
				Or, if you prefer to inspect before running:
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
						<span class="t-cmd">curl -fsSL https://conary.io/install.sh -o install.sh</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">less install.sh</span>
					</div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">sh install.sh</span>
					</div>
				</div>
			</div>
		</div>

		<!-- Distro-specific -->
		<div class="install-block animate-in" style="--stagger: 5">
			<h2>Distribution Packages</h2>
			<p class="install-note">Native packages for each supported distribution.</p>

			<div class="distro-cards">
				<div class="distro-card distro-fedora">
					<div class="distro-badge">Fedora</div>
					<div class="terminal compact">
						<div class="terminal-body">
							<div class="terminal-line">
								<span class="t-prompt">$</span>
								<span class="t-cmd">sudo dnf install conary-release</span>
							</div>
							<div class="terminal-line">
								<span class="t-prompt">$</span>
								<span class="t-cmd">sudo dnf install conary</span>
							</div>
						</div>
					</div>
					<p class="distro-note">Fedora 39+ / RHEL 9+</p>
				</div>

				<div class="distro-card distro-arch">
					<div class="distro-badge">Arch Linux</div>
					<div class="terminal compact">
						<div class="terminal-body">
							<div class="terminal-line">
								<span class="t-prompt">$</span>
								<span class="t-cmd">pacman -S conary</span>
							</div>
						</div>
					</div>
					<p class="distro-note">Available in the AUR</p>
				</div>

				<div class="distro-card distro-ubuntu">
					<div class="distro-badge">Ubuntu / Debian</div>
					<div class="terminal compact">
						<div class="terminal-body">
							<div class="terminal-line">
								<span class="t-prompt">$</span>
								<span class="t-cmd">curl -fsSL https://conary.io/gpg.key | sudo gpg --dearmor -o /etc/apt/keyrings/conary.gpg</span>
							</div>
							<div class="terminal-line">
								<span class="t-prompt">$</span>
								<span class="t-cmd">echo "deb [signed-by=/etc/apt/keyrings/conary.gpg] https://apt.conary.io stable main" | sudo tee /etc/apt/sources.list.d/conary.list</span>
							</div>
							<div class="terminal-line">
								<span class="t-prompt">$</span>
								<span class="t-cmd">sudo apt update && sudo apt install conary</span>
							</div>
						</div>
					</div>
					<p class="distro-note">Ubuntu 22.04+ / Debian 12+</p>
				</div>
			</div>
		</div>

		<!-- Build from source -->
		<div class="install-block animate-in" style="--stagger: 7">
			<h2>Build from Source</h2>
			<p class="install-note">Requires Rust 1.93+ and a C compiler.</p>
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
			<h2>First Steps</h2>
			<p class="install-note">Verify installation and start using Conary.</p>
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
					<div class="terminal-line t-output">conary 0.1.0</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary repo add remi https://packages.conary.io</span>
					</div>
					<div class="terminal-line t-output t-success">Repository added.</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary sync</span>
					</div>
					<div class="terminal-line t-output">Syncing metadata from 1 repository...</div>
					<div class="terminal-line t-output t-success">Sync complete. 68,421 packages available.</div>
					<div class="terminal-line t-blank"></div>
					<div class="terminal-line">
						<span class="t-prompt">$</span>
						<span class="t-cmd">conary install htop</span>
					</div>
					<div class="terminal-line t-output">Resolving dependencies...</div>
					<div class="terminal-line t-output">Installing 1 package (156 KB)</div>
					<div class="terminal-line t-output t-success">Transaction complete.</div>
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

	.install-hint {
		font-size: 0.8125rem;
		color: var(--color-text-muted);
		margin: 1rem 0 0.75rem;
	}

	.terminal {
		background: var(--color-code-bg);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		overflow: hidden;
		box-shadow: var(--shadow-lg);
	}

	.terminal.compact {
		box-shadow: none;
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

	.terminal.compact .terminal-body {
		padding: 0.875rem 1.25rem;
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
