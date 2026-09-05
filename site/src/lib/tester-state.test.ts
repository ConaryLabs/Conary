import { describe, expect, it } from 'vitest';
import { deriveTesterState, readTesterGuidePin } from './tester-state';

const TAG = 'v0.16.1';

function guide(frontmatter: string, body = '\n# Agent-Assisted Tester Loop\n\nBody text mentions v9.9.9 and is never authority.\n'): string {
	return `---\n${frontmatter}\n---${body}`;
}

const PAUSED = guide('last_updated: 2026-08-31\nrevision: 18\nstatus: paused\nsummary: Paused guide');
const ACTIVE = guide(`last_updated: 2026-08-31\nrevision: 19\nstatus: active\ntester_release: ${TAG}\nsummary: Active guide`);

describe('readTesterGuidePin', () => {
	it('reads a paused guide without requiring a release', () => {
		expect(readTesterGuidePin(PAUSED, TAG, false)).toEqual({ status: 'paused', release: undefined });
	});

	it('reads an active guide whose release matches the assigned publication', () => {
		expect(readTesterGuidePin(ACTIVE, TAG, true)).toEqual({ status: 'active', release: TAG });
	});

	it('reads a paused guide that already carries a release', () => {
		const text = guide(`status: paused\ntester_release: ${TAG}`);
		expect(readTesterGuidePin(text, TAG, true)).toEqual({ status: 'paused', release: TAG });
	});

	it('reports unknown only when the guide is absent', () => {
		expect(readTesterGuidePin(undefined, TAG, false)).toEqual({ status: 'unknown', release: undefined });
	});

	it('fails closed on a status that is not exactly paused or active', () => {
		expect(() => readTesterGuidePin(guide('status: resumed'), TAG, true)).toThrow(
			/status must be exactly paused or active/
		);
		expect(() => readTesterGuidePin(guide('status: Active'), TAG, true)).toThrow(
			/status must be exactly paused or active/
		);
		expect(() => readTesterGuidePin(guide('summary: no status at all'), TAG, true)).toThrow(
			/status must be exactly paused or active/
		);
	});

	it('fails closed when the frontmatter block is missing', () => {
		expect(() => readTesterGuidePin('# No frontmatter\n\nstatus: active\n', TAG, true)).toThrow(
			/missing YAML frontmatter/
		);
	});

	it('fails closed when an active guide names no release', () => {
		expect(() => readTesterGuidePin(guide('status: active'), TAG, true)).toThrow(
			/active but names no tester_release/
		);
	});

	it('does not accept a release named only in body text', () => {
		const text = guide('status: active', `\nThe pinned release is ${TAG}.\n`);
		expect(() => readTesterGuidePin(text, TAG, true)).toThrow(/active but names no tester_release/);
	});

	it('fails closed on a malformed tester_release', () => {
		expect(() => readTesterGuidePin(guide('status: paused\ntester_release: latest'), TAG, true)).toThrow(
			/not an exact v\* tag: latest/
		);
		expect(() => readTesterGuidePin(guide('status: paused\ntester_release: 0.16.1'), TAG, true)).toThrow(
			/not an exact v\* tag: 0\.16\.1/
		);
	});

	it('fails closed when the active release differs from the published tag', () => {
		const text = guide('status: active\ntester_release: v0.15.0');
		expect(() => readTesterGuidePin(text, TAG, true)).toThrow(
			/active for v0\.15\.0, launch-status publishes v0\.16\.1/
		);
	});

	it('fails closed when the guide is active while launch-status assigns nothing', () => {
		expect(() => readTesterGuidePin(ACTIVE, TAG, false)).toThrow(
			/active while launch-status assigns no tester authority/
		);
	});

	it('fails closed on a duplicated status key, even with identical values', () => {
		expect(() => readTesterGuidePin(guide('status: paused\nstatus: paused'), TAG, false)).toThrow(
			/duplicate frontmatter key/
		);
	});

	it('fails closed on a duplicated tester_release key', () => {
		const text = guide(`status: active\ntester_release: ${TAG}\ntester_release: ${TAG}`);
		expect(() => readTesterGuidePin(text, TAG, true)).toThrow(/duplicate frontmatter key/);
	});

	it('never opens the loop on conflicting duplicate status values', () => {
		const activeThenPaused = guide(`status: active\ntester_release: ${TAG}\nstatus: paused`);
		expect(() => readTesterGuidePin(activeThenPaused, TAG, true)).toThrow(
			/duplicate frontmatter key/
		);
		const pausedThenActive = guide(`status: paused\nstatus: active\ntester_release: ${TAG}`);
		expect(() => readTesterGuidePin(pausedThenActive, TAG, true)).toThrow(
			/duplicate frontmatter key/
		);
	});

	it('fails closed on any other duplicated frontmatter key', () => {
		expect(() => readTesterGuidePin(guide('revision: 18\nstatus: paused\nrevision: 19'), TAG, false)).toThrow(
			/duplicate frontmatter key/
		);
	});

	it('rejects a duplicated key spelled differently in YAML', () => {
		const quotedThenBare = guide(`"status": paused\nstatus: active\ntester_release: ${TAG}`);
		expect(() => readTesterGuidePin(quotedThenBare, TAG, true)).toThrow(/duplicate frontmatter key/);
		const singleQuoted = guide(`status: paused\n'status': active\ntester_release: ${TAG}`);
		expect(() => readTesterGuidePin(singleQuoted, TAG, true)).toThrow(/duplicate frontmatter key/);
	});

	it('accepts YAML-equivalent scalar spellings of the authority values', () => {
		expect(readTesterGuidePin(guide('status: "paused"'), TAG, false)).toEqual({
			status: 'paused',
			release: undefined
		});
		expect(readTesterGuidePin(guide(`"status": active\ntester_release: '${TAG}'`), TAG, true)).toEqual({
			status: 'active',
			release: TAG
		});
	});

	it('ignores YAML comments', () => {
		const text = guide(`# resumed on 2026-09-05\nstatus: active # loop open\ntester_release: ${TAG}`);
		expect(readTesterGuidePin(text, TAG, true)).toEqual({ status: 'active', release: TAG });
	});

	it('rejects folded or literal multi-line authority values', () => {
		expect(() => readTesterGuidePin(guide(`status: >\n  active\ntester_release: ${TAG}`), TAG, true)).toThrow(
			/status must be exactly paused or active/
		);
		expect(() => readTesterGuidePin(guide(`status: |\n  paused`), TAG, false)).toThrow(
			/status must be exactly paused or active/
		);
		expect(() =>
			readTesterGuidePin(guide(`status: active\ntester_release: >\n  ${TAG}`), TAG, true)
		).toThrow(/not an exact v\* tag/);
	});

	it('rejects a nested mapping or a sequence under an authority key', () => {
		expect(() => readTesterGuidePin(guide('status:\n  value: active'), TAG, true)).toThrow(
			/status must be exactly paused or active/
		);
		expect(() => readTesterGuidePin(guide('status:\n  - active'), TAG, true)).toThrow(
			/status must be exactly paused or active/
		);
		expect(() => readTesterGuidePin(guide(`status: active\ntester_release:\n  tag: ${TAG}`), TAG, true)).toThrow(
			/not an exact v\* tag/
		);
		expect(() => readTesterGuidePin(guide(`status: active\ntester_release: [${TAG}]`), TAG, true)).toThrow(
			/not an exact v\* tag/
		);
	});

	it('rejects frontmatter that is not a mapping or not valid YAML', () => {
		expect(() => readTesterGuidePin(guide('- status: paused'), TAG, false)).toThrow(
			/frontmatter is not a YAML mapping/
		);
		expect(() => readTesterGuidePin(guide('status: [paused'), TAG, false)).toThrow(
			/malformed frontmatter YAML/
		);
	});

	it('ignores status-shaped lines outside the frontmatter block', () => {
		const text = guide('status: paused', '\nstatus: active\ntester_release: v0.16.1\n');
		expect(readTesterGuidePin(text, TAG, true)).toEqual({ status: 'paused', release: undefined });
	});
});

describe('deriveTesterState', () => {
	it('is unassigned whenever launch-status assigns no tester authority', () => {
		expect(deriveTesterState(false, { status: 'paused', release: undefined }, TAG)).toBe('unassigned');
		expect(deriveTesterState(false, { status: 'unknown', release: undefined }, TAG)).toBe('unassigned');
	});

	it('is assigned_guide_paused when assigned but the guide is paused', () => {
		expect(deriveTesterState(true, { status: 'paused', release: undefined }, TAG)).toBe(
			'assigned_guide_paused'
		);
		expect(deriveTesterState(true, { status: 'paused', release: TAG }, TAG)).toBe(
			'assigned_guide_paused'
		);
	});

	it('is assigned_guide_paused when assigned but the guide is absent', () => {
		expect(deriveTesterState(true, { status: 'unknown', release: undefined }, TAG)).toBe(
			'assigned_guide_paused'
		);
	});

	it('is assigned_guide_active only when both agree on the same tag', () => {
		expect(deriveTesterState(true, { status: 'active', release: TAG }, TAG)).toBe(
			'assigned_guide_active'
		);
	});

	it('never opens the loop for an active pin on a different tag', () => {
		expect(deriveTesterState(true, { status: 'active', release: 'v0.15.0' }, TAG)).toBe(
			'assigned_guide_paused'
		);
	});

	it('walks the full pipeline from fixture text to state', () => {
		const paused = readTesterGuidePin(PAUSED, TAG, true);
		expect(deriveTesterState(true, paused, TAG)).toBe('assigned_guide_paused');

		const active = readTesterGuidePin(ACTIVE, TAG, true);
		expect(deriveTesterState(true, active, TAG)).toBe('assigned_guide_active');

		const absent = readTesterGuidePin(undefined, TAG, false);
		expect(deriveTesterState(false, absent, TAG)).toBe('unassigned');
	});
});
