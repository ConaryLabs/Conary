<script lang="ts">
	// The hero graphic is inline rather than an <img> so its labels live in the
	// SVG coordinate space. An external image cannot reach the page's fonts or
	// color tokens, which is why the previous version positioned label text over
	// the artwork with percentage offsets and drifted at untested viewports.
</script>

<svg
	class="boundary-diagram"
	viewBox="-30 0 680 500"
	role="img"
	aria-labelledby="boundary-diagram-title boundary-diagram-desc"
>
	<title id="boundary-diagram-title">Cross-format package execution</title>
	<desc id="boundary-diagram-desc">
		RPM, Debian, and Arch packages each enter one Conary transaction that keeps
		the source package ABI, then execute against typed capabilities on a Fedora,
		Ubuntu, or Arch host.
	</desc>

	<g class="d-column-head">
		<text x="150" y="50" text-anchor="end">SOURCE PACKAGE</text>
		<text x="470" y="50" text-anchor="start">TARGET HOST</text>
	</g>

	<g class="d-rule">
		<path d="M 0 66 H 150" />
		<path d="M 470 66 H 620" />
	</g>

	<g class="d-link-in">
		<path d="M 176 120 C 224 120 224 250 268 250" />
		<path d="M 176 250 H 268" />
		<path d="M 176 380 C 224 380 224 250 268 250" />
	</g>

	<g class="d-link-out">
		<path d="M 352 250 C 396 250 396 120 444 120" />
		<path d="M 352 250 H 444" />
		<path d="M 352 250 C 396 250 396 380 444 380" />
	</g>

	<g class="d-node-source">
		<rect x="160" y="112" width="14" height="14" rx="2" />
		<rect x="160" y="242" width="14" height="14" rx="2" />
		<rect x="160" y="372" width="14" height="14" rx="2" />
	</g>

	<g class="d-node-target">
		<rect x="446" y="112" width="14" height="14" rx="2" />
		<rect x="446" y="242" width="14" height="14" rx="2" />
		<rect x="446" y="372" width="14" height="14" rx="2" />
	</g>

	<g class="d-source">
		<text x="146" y="127" text-anchor="end">.rpm</text>
		<text x="146" y="257" text-anchor="end">.deb</text>
		<text x="146" y="387" text-anchor="end">.pkg.tar.zst</text>
	</g>

	<g class="d-target">
		<text x="474" y="127" text-anchor="start">Fedora 44</text>
		<text x="474" y="257" text-anchor="start">Ubuntu 26.04</text>
		<text x="474" y="387" text-anchor="start">Arch Linux</text>
	</g>

	<circle class="d-core-ring" cx="310" cy="250" r="42" />
	<rect
		class="d-core-mark"
		x="286"
		y="226"
		width="48"
		height="48"
		rx="8"
		transform="rotate(45 310 250)"
	/>

	<g class="d-core-label">
		<text x="310" y="324" text-anchor="middle">CONARY</text>
	</g>

	<g class="d-note">
		<text x="310" y="350" text-anchor="middle">one transaction</text>
		<text x="310" y="438" text-anchor="middle">source ABI preserved · typed target capabilities</text>
		<text x="310" y="464" text-anchor="middle">install · update · remove · rollback</text>
	</g>
</svg>

<style>
	/* The viewBox is padded past the geometry on both sides so the longest labels
	   (.pkg.tar.zst, Ubuntu 26.04) stay inside it at every render width. */
	.boundary-diagram {
		width: 100%;
		height: auto;
	}

	text {
		font-family: var(--font-mono);
	}

	.d-column-head text {
		fill: var(--color-muted);
		font-size: 14px;
		letter-spacing: 0.12em;
	}

	.d-rule path {
		stroke: var(--color-border);
		stroke-width: 1;
	}

	.d-link-in path,
	.d-link-out path {
		fill: none;
		stroke: var(--color-cyan);
		stroke-linecap: round;
	}

	/* Dashed inbound, solid outbound: the source format arrives as an input, the
	   executed transaction leaves as Conary's own. */
	.d-link-in path {
		stroke-width: 2;
		stroke-dasharray: 3 6;
	}

	.d-link-out path {
		stroke-width: 2;
	}

	.d-node-source rect {
		fill: none;
		stroke: var(--color-control-border);
		stroke-width: 2;
	}

	.d-node-target rect {
		fill: none;
		stroke: var(--color-cyan);
		stroke-width: 2;
	}

	.d-source text {
		fill: var(--color-mist);
		font-size: 21px;
	}

	.d-target text {
		fill: var(--color-ivory);
		font-size: 21px;
	}

	.d-core-ring {
		fill: var(--color-field);
		stroke: var(--color-cyan);
		stroke-width: 1.5;
	}

	.d-core-mark {
		fill: var(--color-orange);
	}

	.d-core-label text {
		fill: var(--color-ivory);
		font-size: 17px;
		letter-spacing: 0.16em;
	}

	.d-note text {
		fill: var(--color-muted);
		font-size: 14px;
		letter-spacing: 0.03em;
	}
</style>
