function claude-bedrock --description 'Run Claude Code on Amazon Bedrock instead of the claude.ai subscription'
    # Claude Code only accepts region-prefixed Bedrock ids, so Tokyo uses the
    # jp. inference profiles. Override anything before calling, e.g.
    #   ANTHROPIC_MODEL=jp.anthropic.claude-haiku-4-5-20251001-v1:0 claude-bedrock
    set -lx CLAUDE_CODE_USE_BEDROCK 1
    set -lx AWS_PROFILE personal
    set -q AWS_REGION; or set -lx AWS_REGION ap-northeast-1
    set -lx ANTHROPIC_BEDROCK_REGION_PREFIX (string match -r '^jp' $AWS_REGION >/dev/null; and echo jp; or echo us)
    set -q ANTHROPIC_MODEL; or set -lx ANTHROPIC_MODEL jp.anthropic.claude-sonnet-4-6
    set -q ANTHROPIC_SMALL_FAST_MODEL; or set -lx ANTHROPIC_SMALL_FAST_MODEL jp.anthropic.claude-haiku-4-5-20251001-v1:0
    if not command aws sts get-caller-identity --profile $AWS_PROFILE >/dev/null 2>&1
        echo "claude-bedrock: AWS credentials for '$AWS_PROFILE' are missing or expired." >&2
        echo "  refresh with: aws login --profile $AWS_PROFILE --remote" >&2
        return 1
    end
    command claude $argv
end
