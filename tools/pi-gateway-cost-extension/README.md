# pi-gateway-cost-extension

Shows what the Cloudflare AI Gateway itself says the gateway has cost in the last 24 hours, in
Pi's footer.

Pi's own footer total prices the tokens it sent from the model catalog. The gateway bills from its
own token accounting (cached tokens, its own rates, sampling), so the two numbers differ. This
extension asks the gateway through Cloudflare's GraphQL Analytics API and prints its estimate:

```
gateway $16.7/24h
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
`{"accountId": "..."}`, encrypted with [age](https://github.com/FiloSottile/age) to the owner's
public key in `secrets/recipients.txt`. The extension reads `CLOUDFLARE_ACCOUNT_ID` first and
otherwise runs `scripts/secrets.sh decrypt gateway-cost` once per session. The same secret may hold
a scoped `token`; see below.

That script takes the age identity from the first store that has it, so no file has to be copied
between machines:

1. `AGE_IDENTITY`, a path, when it is set
2. 1Password, `AGE_OP_REF` (default `op://Personal/dotfiles-age-identity/notesPlain`)
3. the macOS keychain, `AGE_KEYCHAIN_SERVICE` (default `dotfiles-age-identity`)
4. `~/.config/age/keys.txt`

On a Mac where neither store has it yet:

```sh
brew install age 1password-cli
op signin                                # or: 1Password > Settings > Developer > Integrate with CLI
./scripts/secrets.sh keygen              # stores the identity, prints its public key
./scripts/secrets.sh identity            # says which store answers
```

`keygen` keeps an existing `~/.config/age/keys.txt` and only adds it to the store, so it also
migrates a machine that predates this. To let another Mac decrypt, add its public key to
`secrets/recipients.txt` and re-encrypt:

```sh
printf '{"accountId":"..."}\n' | ./scripts/secrets.sh encrypt gateway-cost
```

## Token

`CLOUDFLARE_API_TOKEN` wins when it is set. Next is a `token` field in the same encrypted secret,
which is the durable choice: create an API token with **Account Analytics Read** and re-encrypt
`{"accountId":"...","token":"..."}`. Otherwise the OAuth token `wrangler` stores in
`~/.wrangler/config/default.toml` is used, so a machine that runs wrangler needs no extra setup.
That token expires hourly: a 401 runs `wrangler whoami` once to renew it and asks again.
`scripts/gateway-cost.sh` prints the same number for other tools (Herdr.app's limits block), with
the same token order and renewal.

The window is the rolling last 24 hours, the same one the gateway dashboard's "Last 24 hours" view
and `scripts/gateway-cost.sh` use, so the two figures agree. Cloudflare's cost metric is an estimate based on tokens, and its
analytics sample the data, so treat the figure as a trend rather than an invoice.

## Check

```sh
bun install
bun run check
```
