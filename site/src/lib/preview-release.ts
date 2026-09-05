import launchStatus from '../../../docs/roadmaps/launch-status.json';

const version = launchStatus.published_release.version;
const tag = launchStatus.published_release.tag;

/**
 * Typed tester-guide pin.
 *
 * `docs/guides/agent-assisted-tester-loop.md` is the execution authority for
 * the external tester loop, and its YAML frontmatter is the only part of it
 * the site reads. Two keys are authority:
 *
 * - `status`: exactly `paused` or `active`. Any other value fails the build.
 * - `tester_release`: an exact `vMAJOR.MINOR.PATCH` tag. Required when
 *   `status` is `active`; it must equal `launch-status.json`'s published tag,
 *   and launch-status must itself assign tester authority. A mismatch, a
 *   missing tag, or an active guide beside an unassigned launch-status is a
 *   contradictory intermediate state and fails the build rather than
 *   exposing the loop. Body text is never consulted.
 *
 * `status` is `unknown` only when the guide file is absent; the pin is then
 * unassigned.
 */
type TesterGuideStatus = 'paused' | 'active' | 'unknown';

const testerGuideFiles = import.meta.glob('../../../docs/guides/agent-assisted-tester-loop.md', {
	query: '?raw',
	import: 'default',
	eager: true
}) as Record<string, string>;

function readTesterGuidePin(text: string | undefined): {
	status: TesterGuideStatus;
	release: string | undefined;
} {
	if (text === undefined) return { status: 'unknown', release: undefined };

	const frontmatter = text.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/);
	if (!frontmatter) throw new Error('tester guide: missing YAML frontmatter');

	const statusMatch = frontmatter[1].match(/^status: (paused|active)$/m);
	if (!statusMatch) throw new Error('tester guide: status must be exactly paused or active');
	const status = statusMatch[1] as TesterGuideStatus;

	const releaseLine = frontmatter[1].match(/^tester_release: (\S+)$/m);
	const release = releaseLine?.[1];
	if (release !== undefined && !/^v\d+\.\d+\.\d+$/.test(release)) {
		throw new Error(`tester guide: tester_release is not an exact v* tag: ${release}`);
	}

	if (status === 'active') {
		if (release === undefined) throw new Error('tester guide: active but names no tester_release');
		if (release !== tag) {
			throw new Error(`tester guide: active for ${release}, launch-status publishes ${tag}`);
		}
		if (launchStatus.tester_authority.state !== 'assigned') {
			throw new Error('tester guide: active while launch-status assigns no tester authority');
		}
	}

	return { status, release };
}

const testerGuide = readTesterGuidePin(Object.values(testerGuideFiles)[0]);
const testerGuideStatus = testerGuide.status;
const testerGuideActive = testerGuide.status === 'active' && testerGuide.release === tag;

const orgUrl = 'https://github.com/FieldmouseWorks';
const repoUrl = `${orgUrl}/Conary`;
const mainBlobUrl = `${repoUrl}/blob/main`;

/**
 * Project identity and support routes. The GitHub organization is
 * FieldmouseWorks; the product is Conary and conary.io is its site.
 */
export const project = {
	name: 'Conary',
	orgName: 'Fieldmouse Works',
	orgTagline: 'Open tools for closed systems.',
	orgUrl,
	repoUrl,
	cloneUrl: `${repoUrl}.git`,
	// Licensing split decided in issue #900: the Conary client and every library
	// crate are MIT OR Apache-2.0; only Remi, the hosted service, is AGPL.
	license: 'MIT OR Apache-2.0',
	licenseUrl: repoUrl,
	remiLicense: 'AGPL-3.0-or-later',
	issuesUrl: `${repoUrl}/issues`,
	newIssueUrl: `${repoUrl}/issues/new/choose`,
	discussionsUrl: `${repoUrl}/discussions`,
	securityAdvisoryUrl: `${repoUrl}/security/advisories/new`,
	securityPolicyUrl: `${mainBlobUrl}/SECURITY.md`,
	contributingUrl: `${mainBlobUrl}/CONTRIBUTING.md`,
	launchStatusUrl: `${mainBlobUrl}/docs/roadmaps/launch-status.json`,
	sourceSelectionUrl: `${mainBlobUrl}/docs/modules/source-selection.md`,
	packagesUrl: 'https://remi.conary.io',
	maintainerEmail: 'peter@conary.io'
} as const;

export const previewRelease = {
	version,
	tag,
	releaseUrl: `${repoUrl}/releases/tag/${tag}`,
	latestReleaseUrl: `${repoUrl}/releases/latest`,
	downloadBaseUrl: `${repoUrl}/releases/download/${tag}`,
	matrixUrl: `${mainBlobUrl}/docs/operations/release-artifact-matrix.md`,
	testerAuthority: launchStatus.tester_authority.state,
	testerGuideStatus,
	/**
	 * True only once launch-status.json assigns an exact external tester release
	 * and the tester guide's typed frontmatter pin is active for that same tag.
	 */
	testerPinAssigned: launchStatus.tester_authority.state === 'assigned' && testerGuideActive,
	testerAuthorityReason: launchStatus.tester_authority.reason,
	announcementClaim: launchStatus.announcement_claim,
	testerGuideUrl: `${mainBlobUrl}/docs/guides/agent-assisted-tester-loop.md`,
	feedbackUrl: `${repoUrl}/issues/new?template=pre_alpha_feedback.md`,
	bootstrapScriptUrl: 'https://conary.io/install-conary-preview.sh'
} as const;
