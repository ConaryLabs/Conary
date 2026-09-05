import launchStatus from '../../../docs/roadmaps/launch-status.json';
import {
	deriveTesterState,
	readTesterGuidePin,
	type TesterGuideStatus,
	type TesterState
} from './tester-state';

const version = launchStatus.published_release.version;
const tag = launchStatus.published_release.tag;

/**
 * The tester guide is loaded through `import.meta.glob` so an absent file is
 * a value (`status: 'unknown'`) rather than an import error. Only its typed
 * frontmatter keys are authority; see `tester-state.ts`.
 */
const testerGuideFiles = import.meta.glob('../../../docs/guides/agent-assisted-tester-loop.md', {
	query: '?raw',
	import: 'default',
	eager: true
}) as Record<string, string>;

const launchAssigned = launchStatus.tester_authority.state === 'assigned';
const testerGuide = readTesterGuidePin(Object.values(testerGuideFiles)[0], tag, launchAssigned);
const testerGuideStatus: TesterGuideStatus = testerGuide.status;
const testerState: TesterState = deriveTesterState(launchAssigned, testerGuide, tag);
export type { TesterState };

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
	testerState,
	/** launch-status assigns the published release as tester authority. */
	testerAssigned: testerState !== 'unassigned',
	/** The tester guide is active for that same release: the loop is open. */
	loopOpen: testerState === 'assigned_guide_active',
	testerAuthorityReason: launchStatus.tester_authority.reason,
	announcementClaim: launchStatus.announcement_claim,
	testerGuideUrl: `${mainBlobUrl}/docs/guides/agent-assisted-tester-loop.md`,
	feedbackUrl: `${repoUrl}/issues/new?template=pre_alpha_feedback.md`,
	bootstrapScriptUrl: 'https://conary.io/install-conary-preview.sh',
	/**
	 * The signed bootstrap manifest for exactly this release. The script's
	 * default follows releases/latest; the site always passes this URL so the
	 * install it shows is bound to the tag the release matrix and launch-status
	 * authorize, not to whatever was published most recently.
	 */
	bootstrapManifestUrl: `${repoUrl}/releases/download/${tag}/conary-bootstrap-v1.manifest`
} as const;
