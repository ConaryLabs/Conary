#!/usr/bin/env bash
# scripts/release.sh -- Automated release based on conventional commits
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

usage() {
    echo "Usage: $0 [conary|erofs|server|test|all] [--dry-run]"
    echo ""
    echo "Analyze conventional commits since last tag and bump versions."
    echo "  conary   - conary CLI + conary-core (src/, conary-core/)"
    echo "  erofs    - conary-erofs (conary-erofs/)"
    echo "  server   - conary-server (conary-server/)"
    echo "  test     - conary-test (conary-test/)"
    echo "  all      - all groups"
    echo "  --dry-run  Show what would happen without making changes"
    exit 1
}

DRY_RUN=false
RELEASE_RELEASE_GROUPS=()

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        conary|erofs|server|test|all) RELEASE_GROUPS+=("$arg") ;;
        *) usage ;;
    esac
done

[[ ${#RELEASE_GROUPS[@]} -eq 0 ]] && usage

if [[ " ${RELEASE_GROUPS[*]} " == *" all "* ]]; then
    RELEASE_GROUPS=(conary erofs server test)
fi

declare -A TAG_PREFIX=(
    [conary]="v"
    [erofs]="erofs-v"
    [server]="server-v"
    [test]="test-v"
)

declare -A PATH_SCOPES=(
    [conary]="src/ conary-core/"
    [erofs]="conary-erofs/"
    [server]="conary-server/"
    [test]="conary-test/"
)

latest_tag() {
    local group="$1"
    local prefix="${TAG_PREFIX[$group]}"
    git tag -l "${prefix}*" --sort=-version:refname | head -1
}

version_from_tag() {
    local tag="$1" group="$2"
    local prefix="${TAG_PREFIX[$group]}"
    echo "${tag#"$prefix"}"
}

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

determine_bump() {
    local group="$1" since_ref="$2"
    local paths="${PATH_SCOPES[$group]}"
    local level="none"

    local commits
    # shellcheck disable=SC2086
    commits=$(git log "${since_ref}..HEAD" --oneline -- $paths 2>/dev/null || true)

    if [[ -z "$commits" ]]; then
        echo "none"
        return
    fi

    while IFS= read -r line; do
        local subject="${line#* }"

        if [[ "$subject" =~ ^(feat|fix|refactor|perf)(\(.+\))?!: ]] || [[ "$subject" =~ BREAKING\ CHANGE ]]; then
            echo "major"
            return
        fi

        if [[ "$subject" =~ ^feat(\(.+\))?: ]] && [[ "$level" != "major" ]]; then
            level="minor"
        fi

        if [[ "$subject" =~ ^(fix|security|perf)(\(.+\))?: ]] && [[ "$level" == "none" ]]; then
            level="patch"
        fi
    done <<< "$commits"

    echo "$level"
}

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

update_cargo_version() {
    local file="$1" new_version="$2"
    sed -i "0,/^version = \".*\"/s/^version = \".*\"/version = \"${new_version}\"/" "$file"
}

update_packaging_versions() {
    local new_version="$1"
    local today
    today=$(date +"%a %b %d %Y")
    local deb_date
    deb_date=$(date -R)

    # RPM spec
    if [[ -f packaging/rpm/conary.spec ]]; then
        sed -i "s/^Version:.*$/Version:        ${new_version}/" packaging/rpm/conary.spec
        echo "  Updated packaging/rpm/conary.spec"
    fi

    # Arch PKGBUILD
    if [[ -f packaging/arch/PKGBUILD ]]; then
        sed -i "s/^pkgver=.*$/pkgver=${new_version}/" packaging/arch/PKGBUILD
        echo "  Updated packaging/arch/PKGBUILD"
    fi

    # Debian changelog (prepend new entry)
    if [[ -f packaging/deb/debian/changelog ]]; then
        local tmp
        tmp=$(mktemp)
        cat > "$tmp" <<DEBEOF
conary (${new_version}-1) unstable; urgency=medium

  * Release ${new_version}

 -- Conary Contributors <contributors@conary.io>  ${deb_date}

DEBEOF
        cat packaging/deb/debian/changelog >> "$tmp"
        mv "$tmp" packaging/deb/debian/changelog
        echo "  Updated packaging/deb/debian/changelog"
    fi

    # CCS package manifest
    if [[ -f packaging/ccs/ccs.toml ]]; then
        sed -i "s/^version = \".*\"/version = \"${new_version}\"/" packaging/ccs/ccs.toml
        echo "  Updated packaging/ccs/ccs.toml"
    fi
}

for group in "${RELEASE_GROUPS[@]}"; do
    echo "=== Releasing: $group ==="

    local_tag=$(latest_tag "$group")
    if [[ -z "$local_tag" ]]; then
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
        generate_changelog "$group" "$local_tag" "$new_version"
        continue
    fi

    case "$group" in
        conary)
            update_cargo_version "Cargo.toml" "$new_version"
            update_cargo_version "conary-core/Cargo.toml" "$new_version"
            echo "  Updated Cargo.toml and conary-core/Cargo.toml"

            # Update distro packaging versions
            update_packaging_versions "$new_version"
            ;;
        erofs)
            update_cargo_version "conary-erofs/Cargo.toml" "$new_version"
            echo "  Updated conary-erofs/Cargo.toml"
            ;;
        server)
            update_cargo_version "conary-server/Cargo.toml" "$new_version"
            echo "  Updated conary-server/Cargo.toml"
            ;;
        test)
            update_cargo_version "conary-test/Cargo.toml" "$new_version"
            echo "  Updated conary-test/Cargo.toml"
            ;;
    esac

    # Regenerate Cargo.lock to match bumped Cargo.toml versions
    cargo generate-lockfile --quiet
    echo "  Updated Cargo.lock"

    # Format and lint to avoid CI failures from version bumps
    cargo fmt --all --quiet
    echo "  Ran cargo fmt"
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged -- -D warnings 2>/dev/null || true
    echo "  Ran cargo clippy --fix"

    changelog_entry=$(generate_changelog "$group" "$local_tag" "$new_version")
    if [[ -f CHANGELOG.md ]]; then
        tmp=$(mktemp)
        head -5 CHANGELOG.md > "$tmp"
        echo "$changelog_entry" >> "$tmp"
        tail -n +6 CHANGELOG.md >> "$tmp"
        mv "$tmp" CHANGELOG.md
    fi

    git add -A
    git commit -m "chore: release ${new_tag}"
    git tag -a "$new_tag" -m "Release ${new_tag}"

    echo "  [DONE] Released ${new_tag}"
    echo ""
done

echo "=== Release complete ==="
