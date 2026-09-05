/**
 * Which commands need `--yes`, derived from `apps/conary/src/command_risk.rs`.
 *
 * `CommandRiskPolicy::requires_apply_intent` is the authority: a command needs
 * apply intent only when its risk is DestructiveDbMutation,
 * SelectedRootMutation, ActiveHostMutation, or AlwaysLive and it is not a dry
 * run. Everything below is transcribed from the `classify_*` functions in that
 * file. When the policy changes, this list changes with it; do not edit the
 * copy on the routes instead.
 *
 * Hook-only entries (`system adopt --sync-hook`,
 * `system adopt --refresh --quiet --from-sync-hook`) are omitted: they are
 * reserved for native package-manager hooks and are not user commands.
 */
export const commandRisk = {
	source: 'apps/conary/src/command_risk.rs',
	sourceUrl:
		'https://github.com/FieldmouseWorks/Conary/blob/main/apps/conary/src/command_risk.rs',

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
	 * Classed read-only or non-host by the policy even though they write
	 * artifacts (images, signatures, keys, lock files, SBOMs, exports, caches,
	 * build outputs) or publish to a remote. The policy file does not say which
	 * read-only arms write; this group comes from the output arguments and
	 * behaviour of each command definition under `apps/conary/src/cli/`.
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
		'conary cache populate',
		'conary bootstrap (every subcommand except seed --from-adopted)',
		'conary export'
	],

	/**
	 * GenerationBootActivation: an internal boot continuation authorized by the
	 * generation artifact and kernel command line, not by `--yes`.
	 */
	bootActivation: ['conary system generation activate']
} as const;
