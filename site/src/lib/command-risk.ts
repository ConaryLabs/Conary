/**
 * Which commands need `--yes`, derived from `apps/conary/src/command_risk.rs`.
 *
 * Two kinds of group live here and they carry different authority:
 *
 * - The `--yes`-gated groups (`requiresYes`, `builtInIntent`) and the policy's
 *   own no-confirmation classes (`databaseWithoutConfirmation`,
 *   `localStateWithoutConfirmation`, `bootActivation`) are transcribed from
 *   `CommandRiskPolicy::requires_apply_intent` and the `classify_*` arms. They
 *   are complete by construction: a command needs apply intent only when its
 *   risk is DestructiveDbMutation, SelectedRootMutation, ActiveHostMutation,
 *   or AlwaysLive and it is not a dry run.
 * - The side-effect groups (`artifactWritingWithoutConfirmation`,
 *   `databaseWritingReadOnlyClassified`) are known misclassifications: the
 *   policy calls them read-only or non-host, their implementations write.
 *   They are examples found by audit, not a complete list, and are tracked in
 *   the issue below until the classifier is fixed and this projection is
 *   generated from it. Nothing here implies an unlisted command is read-only.
 *
 * Hook-only entries (`system adopt --sync-hook`,
 * `system adopt --refresh --quiet --from-sync-hook`) are omitted: they are
 * reserved for native package-manager hooks and are not user commands.
 */
export const commandRisk = {
	source: 'apps/conary/src/command_risk.rs',
	sourceUrl:
		'https://github.com/FieldmouseWorks/Conary/blob/main/apps/conary/src/command_risk.rs',
	misclassificationIssue: '#918',
	misclassificationIssueUrl: 'https://github.com/FieldmouseWorks/Conary/issues/918',

	/** The one-sentence rule every route states. */
	rule:
		'--yes is required only for commands the risk policy classes as active-host, selected-root, destructive-database, or always-live mutations, and only outside --dry-run. Everything else runs without confirmation even when it changes state.',

	/** Commands whose `--yes` flag the policy checks. */
	requiresYes: [
		'conary install',
		'conary install @collection',
		'conary update',
		'conary update @collection',
		'conary remove',
		'conary autoremove',
		'conary ccs install',
		'conary model apply',
		'conary automation apply',
		'conary system unadopt',
		'conary system restore',
		'conary system native-handoff',
		'conary system takeover',
		'conary system repository-takeover',
		'conary system rebuild-db',
		'conary system db-backup recover',
		'conary system state revert',
		'conary system state rollback',
		'conary system generation build',
		'conary system generation publish',
		'conary system generation switch',
		'conary system generation rollback',
		'conary system generation gc',
		'conary system generation recover',
		'conary system generation recover-db'
	],

	/**
	 * Active-host mutations whose apply intent is hard-coded true: there is no
	 * `--yes` flag to omit, so running them is the confirmation.
	 */
	builtInIntent: [
		'conary self-update',
		'conary try keep',
		'conary try rollback',
		'conary try --activate'
	],

	/** DbMutation: change Conary's database without confirmation. */
	databaseWithoutConfirmation: [
		'conary system adopt --system',
		'conary system adopt --refresh',
		'conary system adopt <pkg>'
	],

	/** LocalStateMutation: change local state without confirmation. */
	localStateWithoutConfirmation: [
		'conary pin',
		'conary unpin',
		'conary new',
		'conary cook',
		'conary try',
		'conary try --watch',
		'conary publish',
		'conary system init',
		'conary system state prune',
		'conary system state create',
		'conary system trigger enable / disable / add / remove / run',
		'conary system redirect add / remove',
		'conary system update-channel set / reset',
		'conary repo add / remove / reset-trust / enable / disable / sync',
		'conary config backup / restore',
		'conary registry update',
		'conary query label add / remove / path / set / link / delegate',
		'conary ccs enhance / init / build / test',
		'conary derive create / patch / override / delete',
		'conary model snapshot / update / publish',
		'conary collection create / add / remove / delete',
		'conary automation configure',
		'conary bootstrap seed --from-adopted',
		'conary provenance register',
		'conary trust init / enable',
		'conary federation add-peer / remove-peer / enable-peer / disable-peer / scan --add'
	],

	/**
	 * Known misclassifications (#918), examples only: classed read-only or
	 * non-host by the policy even though they write artifacts (images,
	 * signatures, keys, lock files, SBOMs, PID files, exports, caches, build
	 * outputs) or publish to a remote. Found by reading the output arguments
	 * and behaviour of the command definitions under `apps/conary/src/cli/`
	 * and `apps/conary/src/commands/`.
	 */
	artifactWritingWithoutConfirmation: [
		'conary system generation export',
		'conary model lock',
		'conary sbom',
		'conary system sbom',
		'conary ccs sign',
		'conary ccs keygen',
		'conary ccs export',
		'conary provenance export',
		'conary trust keygen',
		'conary profile generate',
		'conary profile publish',
		'conary derive build',
		'conary derivation build',
		'conary automation daemon',
		'conary cache populate',
		// bootstrap, per apps/conary/src/commands/bootstrap/{setup,phases,image,run,seed,cleanup}.rs
		// and the Bootstrap constructor in crates/conary-core/src/bootstrap/mod.rs, which
		// create_dir_all's the work directory. Read-only by implementation and therefore
		// not listed: bootstrap check, status, verify-convergence, diff-seeds.
		'conary bootstrap init',
		'conary bootstrap dry-run (creates the work directory, validates only)',
		'conary bootstrap cross-tools',
		'conary bootstrap temp-tools',
		'conary bootstrap system',
		'conary bootstrap config',
		'conary bootstrap tier2',
		'conary bootstrap guest-profile',
		'conary bootstrap image',
		'conary bootstrap run',
		'conary bootstrap resume',
		'conary bootstrap seed',
		'conary bootstrap clean (removes stage trees)',
		'conary export'
	],

	/**
	 * Known misclassifications (#918), examples only: classed read-only by the
	 * policy but write Conary's database as a side effect. From the command
	 * implementations: `crates/conary-core/src/automation/check.rs` persists
	 * `troves.orphan_since` (run by `automation check` and on a schedule by
	 * `automation daemon`); `apps/conary/src/commands/federation.rs`
	 * `cmd_federation_test` updates peer latency and last-seen.
	 */
	databaseWritingReadOnlyClassified: [
		'conary automation check',
		'conary automation daemon',
		'conary federation test'
	],

	/**
	 * GenerationBootActivation: an internal boot continuation authorized by the
	 * generation artifact and kernel command line, not by `--yes`.
	 */
	bootActivation: ['conary system generation activate']
} as const;
