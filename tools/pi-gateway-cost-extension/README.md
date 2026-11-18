# pi-gateway-cost-extension

Shows what the Cloudflare AI Gateway itself says the gateway has cost today, in Pi's footer.

Pi's own footer total prices the tokens it sent from the model catalog. The gateway bills from its
own token accounting (cached tokens, its own rates, sampling), so the two numbers differ. This
extension asks the gateway through Cloudflare's GraphQL Analytics API and prints its estimate:

```
gateway $16.71 today
```

It asks on session start and after a turn, at most once a minute, and keeps the last value if a read
fails. One warning is shown if the answer cannot be read.

## Configuration

In `~/.pi/agent/settings.json` only the gateway name, which is not a secret:

```json
"pi-gateway-cost": {
  "gateway": "pi-agent"
}
```

Without that section, or without a token or an account id, the extension asks nothing and shows
nothing.

## Account id

The account id is kept out of Git in plaintext. `secrets/gateway-cost.age` holds
`{"accountId": "..."}`, encrypted with [age](https://github.com/FiloSottile/age) to the owner's key
at `~/.config/age/keys.txt`. The extension reads `CLOUDFLARE_ACCOUNT_ID` first and decrypts that
file otherwise, once per session.

On a new Mac:

```sh
brew install age
# copy ~/.config/age/keys.txt there (password manager, never Git)
./scripts/secrets.sh decrypt gateway-cost   # check it reads
```

To change the value, or to let another Mac read it, add that Mac's public key to
`secrets/recipients.txt` and re-encrypt:

```sh
./scripts/secrets.sh keygen                                    # prints a machine's public key
printf '{"accountId":"..."}\n' | ./scripts/secrets.sh encrypt gateway-cost
```

## Token

`CLOUDFLARE_API_TOKEN` wins when it is set: create an API token with **Account Analytics Read**.
Otherwise the OAuth token `wrangler` already stores in `~/.wrangler/config/default.toml` is used, so
a machine that runs wrangler needs no extra setup.

The window is today in local time. Cloudflare's cost metric is an estimate based on tokens, and its
analytics sample the data, so treat the figure as a trend rather than an invoice.

## Check

```sh
bun install
bun run check
```
