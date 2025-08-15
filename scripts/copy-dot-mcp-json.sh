#!/bin/bash
#
# copy-dot-mcp-json.sh - Create symbolic link or copy .mcp.json file
#
# Usage:
#   ./copy-dot-mcp-json.sh                    # Create symbolic link to .mcp.json in current directory
#   ./copy-dot-mcp-json.sh -c                 # Copy .mcp.json to current directory
#   ./copy-dot-mcp-json.sh -c /path/to/dest  # Copy .mcp.json to specified destination
#
# Options:
#   -c    Copy file instead of creating symbolic link
#
# Examples:
#   # Create symbolic link in current directory (default)
#   ./copy-dot-mcp-json.sh
#
#   # Copy file to current directory
#   ./copy-dot-mcp-json.sh -c
#
#   # Copy file to specific location
#   ./copy-dot-mcp-json.sh -c ~/projects/myapp/.mcp.json
#

DOT_MCP_JSON_PATH="$(cd $(dirname $0); pwd)/../.mcp.json"
TARGET_PATH="$(pwd)/.mcp.json"
COPY_MODE=false
DESTINATION=""

# Parse options
while getopts ":c" opt; do
    case ${opt} in
        c )
            COPY_MODE=true
            ;;
        \? )
            echo "Usage: $0 [-c] [destination]"
            echo "  -c : Copy file instead of creating symbolic link"
            echo "  destination : Optional destination path (only used with -c)"
            exit 1
            ;;
    esac
done
shift $((OPTIND -1))

# Get destination if provided
if [ $# -gt 0 ]; then
    DESTINATION="$1"
else
    DESTINATION="${TARGET_PATH}"
fi

# Execute based on mode
if [ "${COPY_MODE}" = true ]; then
    # Copy mode
    cp -v ${DOT_MCP_JSON_PATH} "${DESTINATION}"
else
    # Default mode: create symbolic link
    if [ -L "${TARGET_PATH}" ]; then
        echo "Symbolic link already exists: ${TARGET_PATH}"
    else
        ln -snfv ${DOT_MCP_JSON_PATH} ${TARGET_PATH}
    fi
fi
