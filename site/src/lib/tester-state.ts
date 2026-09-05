/**
 * External tester state, derived from two typed inputs and nothing else.
 *
 * 1. `docs/roadmaps/launch-status.json` `tester_authority.state`: whether the
 *    project assigns the published release as external tester authority.
 * 2. `docs/guides/agent-assisted-tester-loop.md` YAML frontmatter: the tester
 *    guide is the loop's execution authority, and only two of its keys are
 *    read:
 *    - `status`: exactly `paused` or `active`. Any other value fails the build.
 *    - `tester_release`: an exact `vMAJOR.MINOR.PATCH` tag. Required when
 *      `status` is `active`; it must equal the published tag, and launch-status
 *      must itself assign tester authority.
 *
 * Body text is never consulted. These functions are pure so the fail-closed
 * branches can be proven with fixture strings.
 */

export type TesterGuideStatus = 'paused' | 'active' | 'unknown';

export type TesterGuidePin = {
	status: TesterGuideStatus;
	release: string | undefined;
};

export type TesterState = 'unassigned' | 'assigned_guide_paused' | 'assigned_guide_active';

const EXACT_TAG = /^v\d+\.\d+\.\d+$/;

/**
 * Read the guide's typed pin from its raw Markdown.
 *
 * Throws on every contradictory combination so a build cannot expose the loop
 * by accident: a status other than `paused`/`active`, a malformed
 * `tester_release`, an active guide with no release, an active guide whose
 * release differs from the published tag, or an active guide while
 * launch-status assigns no tester authority.
 *
 * `status` is `unknown` only when the guide text is absent.
 */
export function readTesterGuidePin(
	text: string | undefined,
	publishedTag: string,
	launchAssigned: boolean
): TesterGuidePin {
	if (text === undefined) return { status: 'unknown', release: undefined };

	const frontmatter = text.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/);
	if (!frontmatter) throw new Error('tester guide: missing YAML frontmatter');

	const statusMatch = frontmatter[1].match(/^status: (paused|active)$/m);
	if (!statusMatch) throw new Error('tester guide: status must be exactly paused or active');
	const status = statusMatch[1] as TesterGuideStatus;

	const releaseLine = frontmatter[1].match(/^tester_release: (\S+)$/m);
	const release = releaseLine?.[1];
	if (release !== undefined && !EXACT_TAG.test(release)) {
		throw new Error(`tester guide: tester_release is not an exact v* tag: ${release}`);
	}

	if (status === 'active') {
		if (release === undefined) throw new Error('tester guide: active but names no tester_release');
		if (release !== publishedTag) {
			throw new Error(`tester guide: active for ${release}, launch-status publishes ${publishedTag}`);
		}
		if (!launchAssigned) {
			throw new Error('tester guide: active while launch-status assigns no tester authority');
		}
	}

	return { status, release };
}

/**
 * Combine the two inputs into one explicit state. Assignment and activation
 * are distinct events, so the middle state is named rather than hidden inside
 * a boolean:
 *
 * - `unassigned`: launch-status assigns no tester authority.
 * - `assigned_guide_paused`: launch-status assigns the published release, but
 *   the guide is paused or absent. Routes name the release and say the loop is
 *   not yet open.
 * - `assigned_guide_active`: both agree on the same tag; the loop is open.
 */
export function deriveTesterState(
	launchAssigned: boolean,
	pin: TesterGuidePin,
	publishedTag: string
): TesterState {
	if (!launchAssigned) return 'unassigned';
	if (pin.status === 'active' && pin.release === publishedTag) return 'assigned_guide_active';
	return 'assigned_guide_paused';
}
