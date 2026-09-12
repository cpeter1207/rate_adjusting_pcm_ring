#!/bin/sh
## @file
## @brief Run one command in a disposable, labeled project quality container.
set -eu

if [ "$#" -lt 2 ]; then
	printf '%s\n' "usage: $0 IMAGE COMMAND [ARG...]" >&2
	exit 2
fi

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
image=$1
shift
scope=$(printf '%s' "$root" | cksum | awk '{print $1}')
project_label='org.rptadvanced.test.project=rate-adjusting-pcm-ring'
scope_label="org.rptadvanced.test.scope=$scope"
name="rate-adjusting-pcm-ring-test-$scope-$$"
pull_image=${RPTADV_CONTAINER_PULL:-1}
lock_dir="$root/.work/quality-container.lock"

cleanup_stale()
{
	stale=$(docker container ls --all --quiet --filter 'label=rpt_advanced.test=true' \
		--filter "label=$project_label" --filter "label=$scope_label")
	[ -n "$stale" ] || return 0
	while IFS= read -r container; do
		[ -n "$container" ] || continue
		state=$(docker container inspect --format '{{.State.Running}}' "$container" 2>/dev/null || true)
		[ "$state" = 'false' ] || continue
		docker container rm "$container" >/dev/null
	done <<EOF
$stale
EOF
}

cleanup_current()
{
	docker container rm --force "$name" >/dev/null 2>&1 || true
	rmdir "$lock_dir" >/dev/null 2>&1 || true
}

cleanup_stale
mkdir -p "$root/.work"
if ! mkdir "$lock_dir" 2>/dev/null; then
	printf '%s\n' "another rate-adjusting-pcm-ring container run owns $root" >&2
	exit 1
fi
trap cleanup_current 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

if [ "$pull_image" = '1' ]; then
	docker image pull "$image" >&2
	image_ref=$(docker image inspect --format '{{index .RepoDigests 0}}' "$image")
	if [ -z "$image_ref" ] || [ "$image_ref" = '<no value>' ]; then
		printf '%s\n' "could not determine the freshly pulled digest for $image" >&2
		exit 1
	fi
	printf '%s\n' "Using $image_ref" >&2
elif [ "$pull_image" = '0' ]; then
	image_ref=$(docker image inspect --format '{{.Id}}' "$image")
	printf '%s\n' "Using existing local image $image ($image_ref)" >&2
else
	printf '%s\n' 'RPTADV_CONTAINER_PULL must be 0 or 1' >&2
	exit 2
fi

host_root=$root
case $(uname -s) in
	MINGW*|MSYS*)
		host_root=$(cd "$root" && pwd -W)
		export MSYS_NO_PATHCONV=1
		;;
esac

docker run --rm --name "$name" --label rpt_advanced.test=true \
	--label "$project_label" --label "$scope_label" \
	--volume "$host_root:/workspace" --workdir /workspace "$image_ref" "$@"
