#!/usr/bin/env bash
# Installs Ubuntu packages on a CI runner without letting one slow mirror
# hold the job (#72: "System libraries" timed out at 15 minutes twice on
# main pushes).
#
#   apt-install.sh <deb cache dir> <package>...
#
# - Debs from an earlier run (actions/cache keeps <deb cache dir>) are
#   copied into apt's archive first, so only new or changed packages are
#   downloaded.
# - `apt-get update` and the install each get a hard time limit and up to
#   three attempts; a later attempt switches from the runner's default
#   mirror to archive.ubuntu.com and back.
# - The downloaded debs are copied back to <deb cache dir> for the cache
#   save step.
set -uo pipefail

cache="$1"
shift
mkdir -p "$cache"

APT=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30
     -o Acquire::http::Pipeline-Depth=0 -o DPkg::Lock::Timeout=120
     -o APT::Keep-Downloaded-Packages=true -o Binary::apt::APT::Keep-Downloaded-Packages=true)

if compgen -G "$cache/*.deb" > /dev/null; then
  echo "apt-install: $(ls "$cache"/*.deb | wc -l) cached debs"
  sudo cp -n "$cache"/*.deb /var/cache/apt/archives/ 2>/dev/null || true
fi

switch_mirror() {
  # The runner's sources point at azure.archive.ubuntu.com; the other
  # attempt uses the main archive. Both deb822 and classic lists.
  local from="$1" to="$2"
  sudo sed -i "s#${from}#${to}#g" /etc/apt/sources.list /etc/apt/sources.list.d/*.list \
    /etc/apt/sources.list.d/*.sources 2>/dev/null || true
}

attempt() {
  local n="$1"
  echo "::group::apt attempt $n"
  timeout 300 sudo apt-get "${APT[@]}" update
  local up=$?
  if [ $up -ne 0 ]; then
    echo "apt-install: update failed or timed out ($up)"
  fi
  timeout 600 sudo DEBIAN_FRONTEND=noninteractive apt-get "${APT[@]}" install -y --no-install-recommends "$@"
  local rc=$?
  echo "::endgroup::"
  return $rc
}

ok=1
for n in 1 2 3; do
  if attempt "$n" "$@"; then
    ok=0
    break
  fi
  echo "apt-install: attempt $n failed"
  if [ "$n" = 1 ]; then
    switch_mirror "http://azure.archive.ubuntu.com" "http://archive.ubuntu.com"
  else
    switch_mirror "http://archive.ubuntu.com" "http://azure.archive.ubuntu.com"
  fi
  sleep $((n * 10))
done

if [ $ok -eq 0 ]; then
  cp -n /var/cache/apt/archives/*.deb "$cache"/ 2>/dev/null || true
  echo "apt-install: done; $(ls "$cache"/*.deb 2>/dev/null | wc -l) debs in the cache dir"
fi
exit $ok
