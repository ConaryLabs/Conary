<script lang="ts">
	import PageIntro from '$lib/components/PageIntro.svelte';
	import PageMeta from '$lib/components/PageMeta.svelte';
	import { previewRelease } from '$lib/preview-release';
</script>

<PageMeta
	title="Conary compared with apt, dnf, pacman, and Nix — Conary"
	description="Compare Conary's cross-distro package model, migration path, and current maturity with apt, dnf, pacman, and Nix."
	path="/compare/"
/>

<PageIntro
	eyebrow="Trade-off matrix"
	title="Compare the operating models, not the checkmarks."
	description="Conary is not the mature choice today. Its near-term bet is that RPM, DEB, and Arch packages can retain their source semantics while one engine owns the transaction on any supported Linux target."
/>

<section class="compare-section">
	<div class="container">
		<div class="compare-block">
			<div class="matrix-intro">
				<div>
					<p class="eyebrow matrix-eyebrow">Capability matrix</p>
					<h2>One row, one defined behavior.</h2>
				</div>
				<div class="matrix-meta">
					<span>Compared on</span>
					<strong>2026-07-25</strong>
					<p>Documentation snapshot · default first-party behavior unless a cell says otherwise</p>
				</div>
			</div>

			<p class="matrix-note">
				Conary labels describe current maturity. Competitor labels distinguish defaults,
				version-gated features, adjacent first-party stacks, and different package models.
				This is not a performance benchmark or an ecosystem-size ranking.
			</p>
			<p class="scroll-cue" aria-hidden="true">Scroll horizontally to compare →</p>

			<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal comparison needs a keyboard focus target) -->
			<div class="compare-table-wrap scroll-region" tabindex="0" role="region" aria-label="Package manager operating-model comparison">
				<table class="compare-table">
					<caption>Documentation-based comparison of package-manager operating models, reviewed 2026-07-25.</caption>
					<thead>
						<tr>
							<th scope="col" class="feature-heading">Defined behavior</th>
							<th scope="col" class="highlight">Conary</th>
							<th scope="col">apt</th>
							<th scope="col">dnf</th>
							<th scope="col">pacman</th>
							<th scope="col">Nix / NixOS</th>
						</tr>
					</thead>
					<tbody>
						<tr>
							<th scope="row" class="feature-name">Dependency selection model</th>
							<td class="highlight"><span class="status built-in">SAT · built-in</span></td>
							<td><span class="status version">APT 3 SAT fallback</span></td>
							<td><span class="status default">SAT · default</span></td>
							<td><span class="status different">non-SAT</span></td>
							<td><span class="status na">evaluated graph</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Direct host-distro repository compatibility</th>
							<td class="highlight"><span class="status different">not direct · conversion</span></td>
							<td><span class="status default">DEB/APT default</span></td>
							<td><span class="status default">RPM-md default</span></td>
							<td><span class="status default">sync repos default</span></td>
							<td><span class="status different">separate Nix repos</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Reversible native tracking without authority transfer</th>
							<td class="highlight"><span class="status preview">preview-supported</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status no">No</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Strict content-addressed package files or outputs</th>
							<td class="highlight"><span class="status limited">package files</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status experimental">experimental outputs</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Binary package-payload delta downloads</th>
							<td class="highlight"><span class="status roadmap">roadmap only</span></td>
							<td><span class="status no">No core support</span></td>
							<td><span class="status no">No in DNF5</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status no">No standard path</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Package-history inverse transaction</th>
							<td class="highlight"><span class="status limited">changeset-limited</span></td>
							<td><span class="status version">APT 3.1+</span></td>
							<td><span class="status built-in">built-in history</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status na">generation switch</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Bootloader-selectable whole-system generations</th>
							<td class="highlight"><span class="status vm">VM-only</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status separate">rpm-ostree stack</span></td>
							<td><span class="status no">No</span></td>
							<td><span class="status default">NixOS default</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Alternate root or foreign-architecture tooling</th>
							<td class="highlight"><span class="status experimental">bootstrap research</span></td>
							<td><span class="status separate">companion workflow</span></td>
							<td><span class="status built-in">built-in</span></td>
							<td><span class="status built-in">built-in</span></td>
							<td><span class="status built-in">different model</span></td>
						</tr>
						<tr>
							<th scope="row" class="feature-name">Cross-target package portability</th>
							<td class="highlight"><span class="status preview">preview core</span></td>
							<td><span class="status different">Debian-family model</span></td>
							<td><span class="status different">RPM-family model</span></td>
							<td><span class="status different">Arch package model</span></td>
							<td><span class="status built-in">broad host support</span></td>
						</tr>
					</tbody>
				</table>
			</div>

			<div class="status-key" aria-label="Status vocabulary">
				<span><i class="key-mark cyan" aria-hidden="true"></i>default, built-in, or preview-supported</span>
				<span><i class="key-mark orange" aria-hidden="true"></i>limited, version-gated, VM-only, or experimental</span>
				<span><i class="key-mark muted" aria-hidden="true"></i>different model, separate stack, roadmap, N/A, or no</span>
			</div>
		</div>

		<section class="definitions-section" aria-labelledby="definitions-title">
			<div class="definitions-heading">
				<p class="eyebrow">Row definitions</p>
				<h2 id="definitions-title">What the matrix is actually measuring.</h2>
			</div>
			<dl class="definitions-list">
				<div><dt>Dependency selection</dt><dd>How the tool chooses a consistent package set from versions, alternatives, providers, and conflicts.</dd></div>
				<div><dt>Direct repository compatibility</dt><dd>Consumes the host distro's native repository metadata and binary package format without conversion.</dd></div>
				<div><dt>Reversible native tracking</dt><dd>Adds and removes another tool's tracking while the native manager keeps package authority and files.</dd></div>
				<div><dt>Strict content addressing</dt><dd>Object identity derives from resulting content, rather than only the build inputs or package filename.</dd></div>
				<div><dt>Binary payload deltas</dt><dd>Reconstructs a new package payload from an old payload; repository-index diffs do not count.</dd></div>
				<div><dt>Inverse package transaction</dt><dd>A built-in history command attempts to reverse recorded package actions; this is not a filesystem snapshot.</dd></div>
				<div><dt>Bootable generations</dt><dd>Retained complete system closures are selectable as boot entries, not merely package-cache downgrades.</dd></div>
				<div><dt>Alternate root or architecture</dt><dd>First-party operation targets a root or architecture different from the running host.</dd></div>
				<div><dt>Cross-target portability</dt><dd>A source package keeps its version, dependency, payload, configuration, and lifecycle ABI on a host whose native package format differs; equivalent-name mapping alone does not count.</dd></div>
			</dl>
		</section>

		<section class="application-section" aria-labelledby="application-title">
			<div class="application-heading">
				<p class="eyebrow">Application semantics</p>
				<h2 id="application-title">“Transactional” does not mean the same thing.</h2>
			</div>
			<div class="application-grid">
				<article><h3>apt / dpkg</h3><p>APT resolves and downloads, then dpkg mutates installed paths. Current APT adds history reversal, but interrupted work can still require repair or completion.</p></article>
				<article><h3>dnf / RPM</h3><p>DNF resolves, verifies, tests, and executes an RPM transaction. History undo and rollback create inverse package actions rather than switching to a retained filesystem image.</p></article>
				<article><h3>pacman / libalpm</h3><p>pacman prepares and checks a transaction, then commits changes in place. Cached packages support manual recovery, not native transaction undo.</p></article>
				<article><h3>Nix / NixOS</h3><p>Nix builds side-by-side store paths and atomically repoints profiles. NixOS retains bootable generations, while live activation can still perform stateful service work.</p></article>
			</div>
		</section>

		<section class="details-section" aria-labelledby="tradeoffs-title">
			<div class="details-heading">
				<p class="eyebrow">Practical trade-offs</p>
				<h2 id="tradeoffs-title">Where Conary fits—and where it does not.</h2>
			</div>
			<div class="details">
				<article class="detail-card">
					<h3>Alongside apt on Ubuntu 26.04 LTS</h3>
					<p>
						APT is the established DEB workflow. APT 3 adds a SAT fallback and version-gated
						history rollback, while dpkg still applies package state in place. Conary can
						install RPM or Arch inputs on the same host without asking apt or dpkg to own
						the transaction.
					</p>
				</article>

				<article class="detail-card">
					<h3>Alongside dnf on Fedora 44</h3>
					<p>
						DNF5 uses libsolv and has built-in history reversal. Fedora stopped generating
						delta RPMs in Fedora 40. Conary adds source-independent DEB and Arch package
						execution, a reversible adoption bridge, and VM-focused generations.
					</p>
				</article>

				<article class="detail-card">
					<h3>Alongside pacman on Arch</h3>
					<p>
						pacman keeps a direct prepare-and-commit model without native transaction undo or
						binary payload deltas. Conary can consume RPM and DEB inputs on Arch while
						retaining their source-native transaction semantics.
					</p>
				</article>

				<article class="detail-card">
					<h3>Compared with Nix and NixOS</h3>
					<p>
						Nix has an established immutable store, atomic profile switching, and a much larger
						package ecosystem; NixOS adds mature bootable system generations. Strict
						content-addressed derivation outputs remain experimental rather than the default.
					</p>
				</article>

				<article class="detail-card">
					<h3>Alongside Flatpak or Snap</h3>
					<p>
						Flatpak and Snap distribute applications through runtime or container boundaries.
						Conary addresses base-system packages, libraries, services, and package authority.
						They solve different layers and can coexist.
					</p>
				</article>

				<article class="detail-card maturity-card">
					<h3>Where Conary is still early</h3>
					<p>
						Conary {previewRelease.version} is an immutable published preview artifact, but
						it is not current tester authority while supported-host fixes remain unreleased.
						Native CCS packages are few, the cross-distro lifecycle matrix still needs wider
						installed-host evidence, generation work is VM-only, and the community and
						operational evidence are small beside established managers.
					</p>
				</article>
			</div>
		</section>

		<section class="sources-section" aria-labelledby="sources-title">
			<div>
				<p class="eyebrow">Method and sources</p>
				<h2 id="sources-title">Claims are tied to named documentation.</h2>
				<p>Third-party extensions are excluded unless a cell names an add-on or separate stack.</p>
			</div>
			<ul class="source-list">
				<li><a href={previewRelease.testerGuideUrl}>Conary paused tester guide <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://github.com/ConaryLabs/Conary/blob/main/docs/modules/source-selection.md">Conary source-selection boundary <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://documentation.ubuntu.com/release-notes/26.04/summary-for-lts-users/">Ubuntu 26.04 APT summary <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://dnf5.readthedocs.io/en/latest/commands/history.8.html">DNF5 history commands <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://dnf5.readthedocs.io/en/stable/changes_from_dnf4.7.html">DNF5 changes from DNF4 <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://man.archlinux.org/man/pacman.8.en">pacman manual <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://nix.dev/manual/nix/stable/package-management/profiles">Nix profiles <span aria-hidden="true">↗</span></a></li>
				<li><a href="https://nixos.org/manual/nixos/stable/">NixOS manual <span aria-hidden="true">↗</span></a></li>
			</ul>
		</section>

		<div class="compare-cta">
			<div>
				<p class="eyebrow">The useful question</p>
				<h2>Would one package engine across distro boundaries solve a real problem?</h2>
			</div>
			<a href="/install/" class="btn btn-primary">Run the bounded preview</a>
		</div>
	</div>
</section>

<style>
	.compare-section {
		padding: var(--section-space) 0 1rem;
	}

	.compare-block,
	.definitions-section,
	.application-section,
	.details-section,
	.sources-section,
	.compare-cta {
		max-width: 1120px;
		margin-inline: auto;
	}

	.compare-block,
	.definitions-section,
	.application-section,
	.details-section,
	.sources-section {
		margin-bottom: clamp(4rem, 9vw, 7rem);
	}

	.matrix-intro,
	.definitions-section,
	.application-section,
	.details-section,
	.sources-section {
		display: grid;
		grid-template-columns: minmax(220px, 0.75fr) minmax(0, 2fr);
		gap: clamp(2rem, 6vw, 5rem);
	}

	.matrix-intro {
		align-items: end;
		margin-bottom: 1rem;
	}

	.matrix-intro h2,
	.definitions-heading h2,
	.application-heading h2,
	.details-heading h2,
	.sources-section h2 {
		margin-bottom: 0;
		font-size: var(--step-section);
	}

	.compare-cta h2 {
		margin-bottom: 0;
		font-size: var(--step-lead);
	}

	.matrix-eyebrow {
		margin-bottom: 0.5rem;
	}

	.matrix-meta {
		justify-self: end;
		max-width: 360px;
		padding-left: 1rem;
		border-left: 2px solid var(--color-orange);
		font-family: var(--font-mono);
	}

	.matrix-meta span {
		display: block;
		color: var(--color-muted);
		font-size: 0.65rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.matrix-meta strong {
		color: var(--color-ivory);
		font-size: 0.85rem;
		font-weight: 500;
	}

	.matrix-meta p {
		margin: 0.35rem 0 0;
		color: var(--color-muted);
		font-family: var(--font-body);
		font-size: 0.78rem;
	}

	.matrix-note {
		max-width: 78ch;
		margin: 0 0 1.25rem;
		color: var(--color-muted);
		font-size: 0.88rem;
	}

	.scroll-cue {
		display: none;
		margin: 0 0 0.55rem;
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.68rem;
		text-align: right;
	}

	.compare-table-wrap {
		border: 1px solid var(--color-control-border);
		background: var(--color-code-bg);
	}

	.compare-table-wrap:focus-visible {
		outline-offset: 3px;
	}

	.compare-table {
		width: 100%;
		min-width: 1050px;
		border-collapse: collapse;
		font-size: 0.8rem;
	}

	.compare-table caption {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	.compare-table th,
	.compare-table td {
		padding: 0.85rem 0.8rem;
		border-bottom: 1px solid var(--color-border);
		text-align: center;
		vertical-align: middle;
	}

	.compare-table thead th {
		color: var(--color-muted);
		background: var(--color-layer);
		font-family: var(--font-mono);
		font-size: 0.66rem;
		font-weight: 500;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.compare-table th.highlight,
	.compare-table td.highlight {
		background: rgb(70 199 211 / 7%);
	}

	.compare-table th.highlight {
		color: var(--color-cyan);
	}

	.compare-table .feature-name,
	.compare-table .feature-heading {
		min-width: 250px;
		color: var(--color-ivory);
		background: var(--color-code-bg);
		font-family: var(--font-body);
		font-size: 0.82rem;
		font-weight: 500;
		letter-spacing: 0;
		text-align: left;
		text-transform: none;
	}

	.compare-table .feature-heading {
		background: var(--color-layer);
		font-family: var(--font-mono);
		font-size: 0.66rem;
		text-transform: uppercase;
	}

	.compare-table tbody tr:last-child th,
	.compare-table tbody tr:last-child td {
		border-bottom: 0;
	}

	.status {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 0.4rem;
		font-family: var(--font-mono);
		font-size: 0.67rem;
		line-height: 1.35;
	}

	.status::before {
		content: '';
		width: 0.5rem;
		height: 0.5rem;
		flex: 0 0 auto;
	}

	.status.default,
	.status.built-in,
	.status.preview {
		color: var(--color-cyan);
	}

	.status.default::before,
	.status.preview::before {
		background: var(--color-cyan);
	}

	.status.built-in::before {
		border: 1px solid var(--color-cyan);
	}

	.status.version,
	.status.limited,
	.status.vm,
	.status.experimental {
		color: var(--color-orange);
	}

	.status.version::before,
	.status.vm::before {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.8);
	}

	.status.limited::before,
	.status.experimental::before {
		border: 1px solid var(--color-orange);
	}

	.status.no,
	.status.na,
	.status.different,
	.status.separate,
	.status.roadmap {
		color: var(--color-muted);
	}

	.status.no::before,
	.status.na::before,
	.status.different::before,
	.status.separate::before,
	.status.roadmap::before {
		width: 0.58rem;
		height: 1px;
		background: var(--color-muted);
	}

	.status-key {
		display: flex;
		flex-wrap: wrap;
		gap: 0.8rem 1.25rem;
		justify-content: flex-end;
		margin-top: 0.9rem;
		color: var(--color-muted);
		font-family: var(--font-mono);
		font-size: 0.64rem;
	}

	.status-key span {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
	}

	.key-mark {
		width: 0.5rem;
		height: 0.5rem;
		flex: 0 0 auto;
	}

	.key-mark.cyan {
		background: var(--color-cyan);
	}

	.key-mark.orange {
		background: var(--color-orange);
		transform: rotate(45deg) scale(0.8);
	}

	.key-mark.muted {
		height: 1px;
		background: var(--color-muted);
	}

	.definitions-heading,
	.application-heading,
	.details-heading {
		align-self: start;
	}

	.definitions-list {
		margin: 0;
		border-top: 1px solid var(--color-border);
	}

	.definitions-list > div {
		display: grid;
		grid-template-columns: minmax(180px, 0.72fr) minmax(0, 1.4fr);
		gap: 1.25rem;
		padding: 1rem 0;
		border-bottom: 1px solid var(--color-border);
	}

	.definitions-list dt {
		color: var(--color-cyan);
		font-family: var(--font-mono);
		font-size: 0.72rem;
	}

	.definitions-list dd {
		margin: 0;
		color: var(--color-mist);
		font-size: 0.9rem;
	}

	.application-grid,
	.details {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 0 clamp(2rem, 5vw, 4.5rem);
	}

	.application-grid article,
	.detail-card {
		padding: 1.6rem 0;
		border-top: 1px solid var(--color-border);
	}

	.application-grid h3,
	.detail-card h3 {
		margin-bottom: 0.7rem;
		font-family: var(--font-body);
		font-size: 1.05rem;
		font-weight: 600;
		letter-spacing: 0;
	}

	.application-grid p,
	.detail-card p {
		margin: 0;
		color: var(--color-mist);
		font-size: 0.93rem;
		line-height: 1.72;
	}

	.maturity-card {
		padding: clamp(1.5rem, 4vw, 2.5rem);
		border: 1px solid var(--color-control-border);
		border-left: 4px solid var(--color-orange);
		background: var(--color-layer);
	}

	.maturity-card h3 {
		color: var(--color-orange);
	}

	.sources-section {
		align-items: start;
	}

	.sources-section > div > p:last-child {
		margin: 0.75rem 0 0;
		color: var(--color-muted);
	}

	.source-list {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 1px;
		margin: 0;
		padding: 1px;
		list-style: none;
		background: var(--color-border);
	}

	.source-list a {
		display: flex;
		align-items: center;
		justify-content: space-between;
		min-height: 50px;
		padding: 0.75rem 0.9rem;
		color: var(--color-mist);
		background: var(--color-code-bg);
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-decoration: none;
	}

	.source-list a:hover {
		color: var(--color-field);
		background: var(--color-cyan);
	}

	.compare-cta {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: 3rem;
		padding: clamp(2rem, 5vw, 4rem);
		border: 1px solid var(--color-control-border);
		background: var(--color-layer);
	}

	.compare-cta .btn {
		flex: 0 0 auto;
	}

	@media (max-width: 760px) {
		.scroll-cue {
			display: block;
		}

		.matrix-intro,
		.definitions-section,
		.application-section,
		.details-section,
		.sources-section {
			grid-template-columns: 1fr;
		}

		.matrix-meta {
			justify-self: start;
		}

		.application-grid,
		.details,
		.source-list {
			grid-template-columns: 1fr;
		}

		.compare-cta {
			align-items: stretch;
			flex-direction: column;
		}
	}

	@media (max-width: 520px) {
		.definitions-list > div {
			grid-template-columns: 1fr;
			gap: 0.4rem;
		}
	}
</style>
