# Automated Versioning Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Automate version bumps across 3 independent crate groups using conventional commit analysis, with doc freshness tracking via YAML frontmatter.

**Architecture:** A shell script (`scripts/release.sh`) analyzes conventional commits since the last git tag per version group, determines the semver bump, updates Cargo.toml files, generates CHANGELOG entries, and creates annotated tags. CLAUDE.md gets conventional commit rules so agents self-enforce the format. Doc versioning is a convention enforced via CLAUDE.md rules.

**Tech Stack:** Bash, git, sed, TOML editing via sed patterns

---

### Task 1: Create the release script

**Files:**
- Create: `scripts/release.sh`

**Context:**
- Existing tag: `v0.1.0` (commit `76b1b21`)
- Version groups: `conary` (src/ + conary-core/), `erofs` (conary-erofs/), `server` (conary-server/)
- Tag format: `v0.2.0` (conary), `erofs-v0.1.1` (erofs), `server-v0.2.0` (server)
- Cargo.toml locations: `Cargo.toml:86`, `conary-core/Cargo.toml:3`, `conary-erofs/Cargo.toml:3`, `conary-server/Cargo.toml:3`

**Step 1: Create scripts directory and write the release script**

```bash
#!/usr/bin/env bash
# scripts/release.sh -- Automated release based on conventional commits
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

usage() {
    echo "Usage: $0 [conary|erofs|server|all] [--dry-run]"
    echo ""
    echo "Analyze conventional commits since last tag and bump versions."
    echo "  conary   - conary CLI + conary-core (src/, conary-core/)"
    echo "  erofs    - conary-erofs (conary-erofs/)"
    echo "  server   - conary-server (conary-server/)"
    echo "  all      - all groups"
    echo "  --dry-run  Show what would happen without making changes"
    exit 1
}

DRY_RUN=false
GROUPS=()

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        conary|erofs|server|all) GROUPS+=("$arg") ;;
        *) usage ;;
    esac
done

[[ ${#GROUPS[@]} -eq 0 ]] && usage

# Expand "all" to individual groups
if [[ " ${GROUPS[*]} " == *" all "* ]]; then
    GROUPS=(conary erofs server)
fi

# Map group -> tag prefix, path scopes, Cargo.toml files
declare -A TAG_PREFIX=(
    [conary]="v"
    [erofs]="erofs-v"
    [server]="server-v"
)

declare -A PATH_SCOPES=(
    [conary]="src/ conary-core/"
    [erofs]="conary-erofs/"
    [server]="conary-server/"
)

# Returns the latest tag for a group, or empty string if none
latest_tag() {
    local group="$1"
    local prefix="${TAG_PREFIX[$group]}"
    # List tags matching prefix, sort by version, take latest
    git tag -l "${prefix}*" --sort=-version:refname | head -1
}

# Parse version from tag (strip prefix)
version_from_tag() {
    local tag="$1" group="$2"
    local prefix="${TAG_PREFIX[$group]}"
    echo "${tag#"$prefix"}"
}

# Bump a semver component
bump_version() {
    local version="$1" level="$2"
    local major minor patch
    IFS='.' read -r major minor patch <<< "$version"
    case "$level" in
        major) echo "$((major + 1)).0.0" ;;
        minor) echo "${major}.$((minor + 1)).0" ;;
        patch) echo "${major}.${minor}.$((patch + 1))" ;;
    esac
}

# Determine bump level from conventional commits
# Returns: major, minor, patch, or none
determine_bump() {
    local group="$1" since_ref="$2"
    local paths="${PATH_SCOPES[$group]}"
    local level="none"

    # Get commits touching this group's paths since the ref
    local commits
    # shellcheck disable=SC2086
    commits=$(git log "${since_ref}..HEAD" --oneline -- $paths 2>/dev/null || true)

    if [[ -z "$commits" ]]; then
        echo "none"
        return
    fi

    while IFS= read -r line; do
        local subject="${line#* }"  # strip hash

        # Check for breaking changes
        if [[ "$subject" =~ ^(feat|fix|refactor|perf)!: ]] || [[ "$subject" =~ BREAKING\ CHANGE ]]; then
            echo "major"
            return
        fi

        # Check for features
        if [[ "$subject" =~ ^feat: ]] && [[ "$level" != "major" ]]; then
            level="minor"
        fi

        # Check for patch-level changes
        if [[ "$subject" =~ ^(fix|security|perf): ]] && [[ "$level" == "none" ]]; then
            level="patch"
        fi
    done <<< "$commits"

    echo "$level"
}

# Generate changelog entries from commits
generate_changelog() {
    local group="$1" since_ref="$2" new_version="$3"
    local paths="${PATH_SCOPES[$group]}"
    local date
    date=$(date +%Y-%m-%d)

    local tag_name="${TAG_PREFIX[$group]}${new_version}"
    echo ""
    echo "## [${tag_name}] - ${date}"
    echo ""

    local -a features=() fixes=() security=() perf=() other=()

    # shellcheck disable=SC2086
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        local subject="${line#* }"

        if [[ "$subject" =~ ^feat!?: ]]; then
            features+=("- ${subject#*: }")
        elif [[ "$subject" =~ ^fix: ]]; then
            fixes+=("- ${subject#*: }")
        elif [[ "$subject" =~ ^security: ]]; then
            security+=("- ${subject#*: }")
        elif [[ "$subject" =~ ^perf: ]]; then
            perf+=("- ${subject#*: }")
        elif [[ "$subject" =~ ^(refactor|test|chore|docs): ]]; then
            : # skip non-user-facing
        else
            other+=("- ${subject}")
        fi
    done < <(git log "${since_ref}..HEAD" --oneline -- $paths 2>/dev/null || true)

    if [[ ${#features[@]} -gt 0 ]]; then
        echo "### Added"
        printf '%s\n' "${features[@]}"
        echo ""
    fi
    if [[ ${#fixes[@]} -gt 0 ]]; then
        echo "### Fixed"
        printf '%s\n' "${fixes[@]}"
        echo ""
    fi
    if [[ ${#security[@]} -gt 0 ]]; then
        echo "### Security"
        printf '%s\n' "${security[@]}"
        echo ""
    fi
    if [[ ${#perf[@]} -gt 0 ]]; then
        echo "### Performance"
        printf '%s\n' "${perf[@]}"
        echo ""
    fi
    if [[ ${#other[@]} -gt 0 ]]; then
        echo "### Other"
        printf '%s\n' "${other[@]}"
        echo ""
    fi
}

# Update version in a Cargo.toml file
update_cargo_version() {
    local file="$1" new_version="$2"
    # Match the first version = "..." line in [package] section
    sed -i "0,/^version = \".*\"/s/^version = \".*\"/version = \"${new_version}\"/" "$file"
}

# Main release loop
for group in "${GROUPS[@]}"; do
    echo "=== Releasing: $group ==="

    local_tag=$(latest_tag "$group")
    if [[ -z "$local_tag" ]]; then
        # No tag for this group yet -- use the repo-wide v0.1.0 as baseline
        local_tag="v0.1.0"
        current_version="0.1.0"
    else
        current_version=$(version_from_tag "$local_tag" "$group")
    fi

    echo "  Current: ${TAG_PREFIX[$group]}${current_version} (tag: ${local_tag})"

    level=$(determine_bump "$group" "$local_tag")

    if [[ "$level" == "none" ]]; then
        echo "  No version-bumping commits since ${local_tag}. Skipping."
        echo ""
        continue
    fi

    new_version=$(bump_version "$current_version" "$level")
    new_tag="${TAG_PREFIX[$group]}${new_version}"

    echo "  Bump: ${level} -> ${new_version}"
    echo "  Tag: ${new_tag}"

    if [[ "$DRY_RUN" == "true" ]]; then
        echo "  [DRY RUN] Would update Cargo.toml files and create tag ${new_tag}"
        echo ""
        # Show what changelog would look like
        generate_changelog "$group" "$local_tag" "$new_version"
        continue
    fi

    # Update Cargo.toml files
    case "$group" in
        conary)
            update_cargo_version "Cargo.toml" "$new_version"
            update_cargo_version "conary-core/Cargo.toml" "$new_version"
            echo "  Updated Cargo.toml and conary-core/Cargo.toml"
            ;;
        erofs)
            update_cargo_version "conary-erofs/Cargo.toml" "$new_version"
            echo "  Updated conary-erofs/Cargo.toml"
            ;;
        server)
            update_cargo_version "conary-server/Cargo.toml" "$new_version"
            echo "  Updated conary-server/Cargo.toml"
            ;;
    esac

    # Generate and prepend changelog
    changelog_entry=$(generate_changelog "$group" "$local_tag" "$new_version")
    if [[ -f CHANGELOG.md ]]; then
        # Insert after the header line (line 5, after the format note)
        local tmp
        tmp=$(mktemp)
        head -5 CHANGELOG.md > "$tmp"
        echo "$changelog_entry" >> "$tmp"
        tail -n +6 CHANGELOG.md >> "$tmp"
        mv "$tmp" CHANGELOG.md
    fi

    # Commit and tag
    git add -A
    git commit -m "chore: release ${new_tag}"
    git tag -a "$new_tag" -m "Release ${new_tag}"

    echo "  [DONE] Released ${new_tag}"
    echo ""
done

echo "=== Release complete ==="
```

**Step 2: Make it executable**

```bash
chmod +x scripts/release.sh
```

**Step 3: Test with --dry-run**

Run: `./scripts/release.sh all --dry-run`

Expected: Shows current versions, commit analysis, bump levels, and what changelog entries would be generated. No files modified.

**Step 4: Commit**

```bash
git add scripts/release.sh
git commit -m "feat: Add automated release script with conventional commit analysis"
```

---

### Task 2: Add conventional commit rules to CLAUDE.md

**Files:**
- Modify: `CLAUDE.md:14-25`

**Step 1: Read CLAUDE.md**

Read the file to confirm current content around the Core Principles section.

**Step 2: Add Commit Convention section after Core Principles**

After line 25 (the Rust Standards line), add a new section:

```markdown

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/). Every commit message MUST start with a type prefix:

| Prefix | When to use | Version bump |
|--------|-------------|-------------|
| `feat:` | New feature or capability | Minor |
| `fix:` | Bug fix | Patch |
| `docs:` | Documentation only | None |
| `refactor:` | Code restructure, no behavior change | None |
| `test:` | Test additions or changes | None |
| `chore:` | Build, tooling, dependencies | None |
| `security:` | Security fix | Patch |
| `perf:` | Performance improvement | Patch |

Add `!` after the type for breaking changes: `feat!: remove legacy API`.

Scopes are optional: `feat(resolver): add SAT backtracking`.

**Release:** Run `./scripts/release.sh [conary|erofs|server|all]` to auto-bump versions, update CHANGELOG.md, and tag. Use `--dry-run` to preview.
```

Use the Edit tool to insert this after the `**Rust Standards**` line and before `## Architecture Glossary`.

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: Add conventional commit rules to CLAUDE.md"
```

---

### Task 3: Add doc versioning rules to CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (add to Core Principles or after Tool Selection)

**Step 1: Read CLAUDE.md**

Confirm current content near the Tool Selection section.

**Step 2: Add Doc Versioning section**

After the Tool Selection section (after line 41), add:

```markdown

## Doc Versioning

When modifying files in `docs/`, add or update a YAML frontmatter header:

```yaml
---
last_updated: 2026-03-06
revision: 1
summary: Brief description of what changed
---
```

- `last_updated`: Set to today's date
- `revision`: Increment on meaningful updates (not typo fixes). Start at 1 for new docs.
- `summary`: One line describing the most recent change
- **Excluded files:** ROADMAP.md, CHANGELOG.md, CONTRIBUTING.md, files in `docs/plans/`
```

Use the Edit tool to insert this.

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: Add doc versioning convention to CLAUDE.md"
```

---

### Task 4: Add frontmatter to existing key docs

**Files:**
- Modify: `docs/conaryopedia-v2.md` (add frontmatter)
- Modify: `docs/ARCHITECTURE.md` (add frontmatter)

**Step 1: Read the first 5 lines of each doc**

Confirm no existing frontmatter.

**Step 2: Add YAML frontmatter to conaryopedia-v2.md**

Prepend to the file:

```yaml
---
last_updated: 2026-03-06
revision: 1
summary: Comprehensive technical guide covering all Conary subsystems
---
```

**Step 3: Add YAML frontmatter to docs/ARCHITECTURE.md**

Prepend to the file:

```yaml
---
last_updated: 2026-03-06
revision: 1
summary: System design overview with module structure and data flow
---
```

**Step 4: Commit**

```bash
git add docs/conaryopedia-v2.md docs/ARCHITECTURE.md
git commit -m "docs: Add version frontmatter to key documentation files"
```

---

### Task 5: Create initial version tags for erofs and server groups

**Files:** None (git operations only)

**Context:** The existing `v0.1.0` tag covers the conary+core group. The erofs and server groups need their own baseline tags so the release script has a starting point for each.

**Step 1: Create baseline tags**

```bash
git tag -a "erofs-v0.1.0" v0.1.0 -m "Baseline: conary-erofs v0.1.0"
git tag -a "server-v0.1.0" v0.1.0 -m "Baseline: conary-server v0.1.0"
```

These point to the same commit as `v0.1.0` — establishing the baseline for future independent versioning.

**Step 2: Verify tags**

```bash
git tag -l
```

Expected output includes: `erofs-v0.1.0`, `server-v0.1.0`, `v0.1.0`

---

### Task 6: Test the release script end-to-end

**Step 1: Run dry-run for all groups**

```bash
./scripts/release.sh all --dry-run
```

Expected: Each group shows commits since its tag, determines bump level, shows proposed changelog. The conary group should show `feat:` and `fix:` commits since `v0.1.0` and propose a minor bump (0.1.0 -> 0.2.0). The erofs and server groups should show their respective commits.

**Step 2: Verify no files were modified**

```bash
git status
```

Expected: clean working tree.

**Step 3: Verify cargo build still works**

```bash
cargo build
```

Expected: success.
