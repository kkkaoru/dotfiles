# Cloudflare billable usage via `cf`. cf's OAuth login cannot hold Billing Read,
# so pass the horse-racing-data debug token for this one call only.
function cfbill --description 'Cloudflare billable usage (cf billing usage get-v1)'
    set -l env_file ~/ghq/github.com/kkkaoru/horse-racing-data/.env
    set -l token (string replace -r '^CLOUDFLARE_DEBUG_TOKEN=["\']?([^"\']*)["\']?$' '$1' (grep '^CLOUDFLARE_DEBUG_TOKEN=' $env_file))
    if test -z "$token"
        echo "cfbill: CLOUDFLARE_DEBUG_TOKEN not found in $env_file" >&2
        return 1
    end
    CLOUDFLARE_API_TOKEN=$token cf billing usage get-v1 $argv
end
