#!/usr/bin/env bash
# Promote a release to stable: fast-forward the stable branch to the tag and mark its release Latest.
# Run locally (not in Actions): the Actions token may not push commits that touch .github/workflows.
set -euo pipefail

tag="${1:-}"
[[ "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: $0 vX.Y.Z" >&2; exit 1; }

git fetch --quiet origin --tags
commit="$(git rev-parse -q --verify "refs/tags/${tag}^{commit}")" || { echo "no tag ${tag}" >&2; exit 1; }
gh release view "${tag}" >/dev/null || { echo "tag ${tag} has no GitHub release yet; wait for the Release workflow" >&2; exit 1; }

# No --force: git refuses unless this is a fast-forward, so stable never moves backwards or sideways.
git push origin "${commit}:refs/heads/stable"
gh release edit "${tag}" --prerelease=false --latest
echo "stable -> ${tag} (${commit})"
