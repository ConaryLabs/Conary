import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const buildRoot = new URL('../build/', import.meta.url);
const pages = [
	{
		file: 'index.html',
		title: 'Conary — One reversible package model for Linux',
		canonical: 'https://conary.io/',
		marker: 'An RPM that installs on Ubuntu. A DEB that installs on Fedora.'
	},
	{
		file: 'features/index.html',
		title: 'Product vision and features — Conary',
		canonical: 'https://conary.io/features/',
		marker: 'The package manager is only the beginning.'
	},
	{
		file: 'install/index.html',
		title: 'Conary tester release status — Conary',
		canonical: 'https://conary.io/install/',
		marker: 'Wait for the next verified tester release.'
	},
	{
		file: 'compare/index.html',
		title: 'Conary compared with apt, dnf, pacman, and Nix — Conary',
		canonical: 'https://conary.io/compare/',
		marker: 'Compare the operating models, not the checkmarks.'
	},
	{
		file: 'about/index.html',
		title: 'About Conary — Conary',
		canonical: 'https://conary.io/about/',
		marker: 'An old packaging idea, rebuilt from scratch.'
	},
	{
		file: 'contact/index.html',
		title: 'Contact — Conary',
		canonical: 'https://conary.io/contact/',
		marker: 'Do not open a public issue for a vulnerability'
	}
];

function fail(message) {
	console.error(`ERROR: ${message}`);
	process.exitCode = 1;
}

function count(html, pattern) {
	return [...html.matchAll(pattern)].length;
}

for (const page of pages) {
	const url = new URL(page.file, buildRoot);
	if (!existsSync(url)) {
		fail(`missing prerendered page: build/${page.file}`);
		continue;
	}

	const html = readFileSync(url, 'utf8');
	if (!html.includes(`<title>${page.title}</title>`)) fail(`${page.file}: missing route title`);
	if (!html.includes(page.marker)) fail(`${page.file}: missing prerendered body marker`);
	if (!html.includes(`rel="canonical" href="${page.canonical}"`)) fail(`${page.file}: missing canonical URL`);
	if (count(html, /<title>/g) !== 1) fail(`${page.file}: expected exactly one title`);
	if (count(html, /<meta name="description"/g) !== 1) fail(`${page.file}: expected exactly one description`);
	if (count(html, /<link rel="canonical"/g) !== 1) fail(`${page.file}: expected exactly one canonical link`);
	if (count(html, /<meta property="og:title"/g) !== 1) fail(`${page.file}: expected exactly one Open Graph title`);
	if (count(html, /<meta name="twitter:title"/g) !== 1) fail(`${page.file}: expected exactly one Twitter title`);
}

const notFoundUrl = new URL('404.html', buildRoot);
if (!existsSync(notFoundUrl)) {
	fail('missing prerendered page: build/404.html');
} else {
	const html = readFileSync(notFoundUrl, 'utf8');
	if (!html.includes('<title>Page not found — Conary</title>')) fail('404.html: missing route title');
	if (!html.includes('This path is outside the current map.')) fail('404.html: missing branded error body');
	if (!html.includes('name="robots" content="noindex"')) fail('404.html: missing noindex metadata');
	if (count(html, /<link rel="canonical"/g) !== 0) fail('404.html: should not declare a canonical URL');
	if (!/(?:src|href)="\/_app\//.test(html)) fail('404.html: application assets must use root-relative URLs');
	if (/(?:src|href)="\.\.?\/_app\//.test(html)) fail('404.html: application assets must not be request-relative');
}

if (!process.exitCode) {
	console.log(`Verified ${pages.length + 1} prerendered HTML pages in ${join('site', 'build')}.`);
}
