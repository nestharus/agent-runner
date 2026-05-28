#!/usr/bin/env sh
set -eu

cat >/dev/null
printf '{"contract":"oulipoly.provider/v1","request_id":"request-example-001","ok":true,"result":{"provider_id":"fake-provider","display_name":"Fake Provider","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{"launch":true,"policy":false,"quota":false,"session":false,"terminal":false,"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false},"concurrency":{"safe_for_parallel_invocation":true,"state_locking":"none"}}}\n'
