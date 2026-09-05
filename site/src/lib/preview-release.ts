import launchStatus from '../../../docs/roadmaps/launch-status.json';

const version = launchStatus.published_release.version;
const tag = launchStatus.published_release.tag;

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
	/** True only once launch-status.json assigns an exact external tester release. */
	testerPinAssigned: launchStatus.tester_authority.state === 'assigned',
	testerAuthorityReason: launchStatus.tester_authority.reason,
	announcementClaim: launchStatus.announcement_claim,
	testerGuideUrl: `${mainBlobUrl}/docs/guides/agent-assisted-tester-loop.md`,
	feedbackUrl: `${repoUrl}/issues/new?template=pre_alpha_feedback.md`,
	bootstrapScriptUrl: 'https://conary.io/install-conary-preview.sh'
} as const;
