# GitHub Actions Native Package Release Pipeline

**Goal:** On `v*` tag push, build RPM/DEB/Arch/CCS packages in GitHub Actions containers, create a GitHub Release with all artifacts, and deploy to Remi for the self-update channel.

**Trigger:** Push of `v*` tag.

## Architecture

Single workflow with a matrix build job for native packages, followed by a release job that creates the GitHub Release and deploys to Remi. Replaces the current raw-binary release job in `ci.yml` and eliminates the need for `publish-release.sh` on Forge.

Native packages are bootstrappers -- they get a skeleton Conary on a supported distro so the user can access the repo and self-update from there. CCS via Remi is the primary distribution channel.

## Jobs

1. **build-packages** (matrix: rpm, deb, arch, ccs) -- Each builds in its distro container using the existing `packaging/*/build.sh` scripts. Uploads artifacts via `actions/upload-artifact`.
2. **release** (depends on build-packages) -- Downloads all artifacts, generates SHA256SUMS, creates GitHub Release with assets via `softprops/action-gh-release`, SSHes to Remi to upload CCS + packages and update the self-update endpoint.

## Build Matrix

| Package | Container | Build approach | Output |
|---------|-----------|----------------|--------|
| RPM | `registry.fedoraproject.org/fedora:43` | rpmbuild with vendored sources | `conary-*.rpm` |
| DEB | `docker.io/library/ubuntu:24.04` | dpkg-buildpackage with vendored sources | `conary_*.deb` |
| Arch | `docker.io/library/archlinux:latest` | makepkg with vendored sources | `conary-*.pkg.tar.zst` |
| CCS | Ubuntu 24.04 (runner default) | `cargo build --release -p conary` + `conary ccs build` | `conary-*.ccs` |

Each matrix job uses the container image as the job's `container:` directive and runs build commands directly (no Podman-in-Docker).

## Secrets

- `REMI_SSH_KEY` -- SSH private key for `root@ssh.conary.io`
- `GITHUB_TOKEN` -- automatic, used for GitHub Release creation

## Remi Deployment

The release job SSHes to Remi to:
- Upload CCS to `/conary/self-update/conary-${VERSION}.ccs`
- Upload all packages to `/conary/releases/${VERSION}/`
- Generate SHA256SUMS in the release directory
- Update `/conary/releases/latest` symlink
- Smoke test: verify self-update API reports the correct version

## What Changes

- **New:** `.github/workflows/release.yml` -- dedicated release workflow
- **Modify:** `.github/workflows/ci.yml` -- remove the existing release job (no more raw binaries)
- **Modify:** `packaging/*/build.sh` -- ensure they work without `--podman` for direct container execution
- **Simplify:** `.forgejo/workflows/release.yaml` -- just verify the release landed (health check)
- **Retire:** `scripts/publish-release.sh` -- GitHub Actions replaces it

## What Stays

- `scripts/release.sh` -- version bumping + tagging (run locally before push)
- Forge CI gate, integration tests, E2E tests, remi-health checks
- All existing Containerfiles (used by both local --podman builds and GH Actions)
