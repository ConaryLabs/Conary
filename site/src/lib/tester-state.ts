import { FAILSAFE_SCHEMA, YAMLException, load } from 'js-yaml';

/**
 * External tester state, derived from two typed inputs and nothing else.
 *
 * 1. `docs/roadmaps/launch-status.json` `tester_authority.state`: whether the
 *    project assigns the published release as external tester authority.
 * 2. `docs/guides/agent-assisted-tester-loop.md` YAML frontmatter: the tester
 *    guide is the loop's execution authority, and only two of its keys are
 *    read:
 *    - `status`: exactly the scalar string `paused` or `active`. Any other
 *      value, including a nested mapping, a sequence, or a folded block, fails
 *      the build.
 *    - `tester_release`: exactly a `vMAJOR.MINOR.PATCH` scalar. Required when
 *      `status` is `active`; it must equal the published tag, and launch-status
 *      must itself assign tester authority.
 *
 * The frontmatter is parsed as YAML (js-yaml, failsafe schema, so every scalar
 * stays a string) and must be a single mapping; the parser rejects duplicated
 * keys in any spelling. Body text is never consulted. These functions are pure
 * so the fail-closed branches can be proven with fixture strings.
 */

export type TesterGuideStatus = 'paused' | 'active' | 'unknown';

export type TesterGuidePin = {
	status: TesterGuideStatus;
	release: string | undefined;
};

export type TesterState = 'unassigned' | 'assigned_guide_paused' | 'assigned_guide_active';

const EXACT_TAG = /^v\d+\.\d+\.\d+$/;

/**
 * Parse the frontmatter block as one YAML mapping. Duplicated keys, in any
 * YAML spelling, and anything that is not a mapping are rejected; the parser
 * never silently picks a first or last occurrence.
 */
function readFrontmatterMapping(block: string): Record<string, unknown> {
	let document: unknown;
	try {
		document = load(block, { schema: FAILSAFE_SCHEMA });
	} catch (error) {
		if (error instanceof YAMLException && /duplicated mapping key/.test(error.reason)) {
			throw new Error(`tester guide: duplicate frontmatter key (${error.reason})`);
		}
		const reason = error instanceof YAMLException ? error.reason : String(error);
		throw new Error(`tester guide: malformed frontmatter YAML (${reason})`);
	}
	if (document === null || typeof document !== 'object' || Array.isArray(document)) {
		throw new Error('tester guide: frontmatter is not a YAML mapping');
	}
	return document as Record<string, unknown>;
}

/**
 * Read the guide's typed pin from its raw Markdown.
 *
 * Throws on every contradictory combination so a build cannot expose the loop
 * by accident: a missing frontmatter block, frontmatter that is not valid YAML
 * or not a single mapping, any key that appears more than once in any YAML
 * spelling (with the same or conflicting values), a `status` that is not
 * exactly the scalar `paused`/`active`, a `tester_release` that is not an
 * exact `v*` scalar, an active guide with no release, an active guide whose
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

	const mapping = readFrontmatterMapping(frontmatter[1]);

	const statusValue = mapping.status;
	if (statusValue !== 'paused' && statusValue !== 'active') {
		throw new Error('tester guide: status must be exactly paused or active');
	}
	const status: TesterGuideStatus = statusValue;

	const releaseValue = mapping.tester_release;
	if (releaseValue !== undefined && (typeof releaseValue !== 'string' || !EXACT_TAG.test(releaseValue))) {
		throw new Error(`tester guide: tester_release is not an exact v* tag: ${String(releaseValue)}`);
	}
	const release = releaseValue as string | undefined;

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
