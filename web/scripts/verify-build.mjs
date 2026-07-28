import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const buildRoot = new URL('../build/', import.meta.url);
const indexPath = join(buildRoot.pathname, 'index.html');

function fail(message) {
	console.error(`ERROR: ${message}`);
	process.exitCode = 1;
}

if (!existsSync(indexPath)) {
	fail('Missing web/build/index.html');
} else {
	const html = readFileSync(indexPath, 'utf8');
	const expectedShell = [
		'<title>Conary Package Index</title>',
		'<meta name="theme-color" content="#0a1224"',
		'<link rel="icon" href="/favicon.svg" type="image/svg+xml"',
		'Browse public package metadata and policy-gated conversion evidence served by Conary Remi.'
	];

	for (const marker of expectedShell) {
		if (!html.includes(marker)) {
			fail(`web/build/index.html is missing ${JSON.stringify(marker)}`);
		}
	}
}

function collectJavaScript(directory) {
	const contents = [];
	for (const entry of readdirSync(directory)) {
		const path = join(directory, entry);
		if (statSync(path).isDirectory()) {
			contents.push(...collectJavaScript(path));
		} else if (path.endsWith('.js')) {
			contents.push(readFileSync(path, 'utf8'));
		}
	}
	return contents;
}

const clientJavaScript = collectJavaScript(join(buildRoot.pathname, '_app')).join('\n');
for (const marker of [
	'Package evidence, across distributions.',
	'One layer in a larger system',
	'Public package metadata and policy-gated conversion evidence served by Remi.'
]) {
	if (!clientJavaScript.includes(marker)) {
		fail(`Generated client routes are missing ${JSON.stringify(marker)}`);
	}
}

const assets = [
	'brand/conary-mark.svg',
	'brand/conary-social.svg',
	'brand/conary-social.png',
	'favicon.svg',
	'favicon.ico',
	'favicon-32.png',
	'apple-touch-icon.png'
];

for (const asset of assets) {
	if (!existsSync(join(buildRoot.pathname, asset))) {
		fail(`Missing generated package-index asset: ${asset}`);
	}
}

if (!process.exitCode) {
	console.log(`Verified package-index shell, client copy, and ${assets.length} branded assets in web/build.`);
}
