# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

### Setup Commands
- `./create-symlinks.sh` - Create symlinks for all dotfiles to $HOME directory (skips .git and .tool-versions)
- `./init-ssh.sh` - Initialize SSH configuration
- `bun install` - Install Node.js dependencies including MCP servers

### Package Management  
- `bun install` - Install dependencies
- `bun add <package>` - Add new dependency
- `bunx <package>` - Execute package without installing

### Script Commands
- `./scripts/create-symlinks.sh` - Alternative symlink creation script
- `./scripts/copy-dot-mcp-json.sh` - Copy MCP configuration
- `./scripts/bun-install-global.sh` - Install global Bun packages
- `./scripts/tmux-three-panes.sh` - Setup tmux with three panes

## Architecture

This is a personal dotfiles repository for macOS development environment configuration, containing:

### Core Configuration Files
- **Shell**: Fish shell with oh-my-fish framework (`.config/fish/`)
  - `config.fish` - Main configuration with Homebrew, paths, and aliases
  - `aliases.fish` - Command aliases
  - `envs.fish` - Environment variables
  - `binds.fish` - Key bindings
  - `path.fish` - PATH configuration

- **Version Management**: 
  - anyenv for multi-language version management (`.config/anyenv/`)
  - mise for modern version management (`.config/mise/`)
  - Default npm packages configuration (`.default-npm-packages`)

- **Development Tools**:
  - Git configuration (`.gitconfig`, `.gitignore_global`)
  - tmux configuration (`.tmux.conf`, `.tmux.session.conf`)
  - Vim configuration (`.vimrc`)
  - VSCode settings (`.vscode/settings.json`)

### MCP (Model Context Protocol) Configuration
The `.mcp.json` file configures various MCP servers:
- **context7** - Documentation server (HTTP type)
- **brave-search** - Web search capabilities
- **motherduck** - DuckDB database server
- **gemini-cli** - Gemini AI integration
- **playwright** - Browser automation
- **serena** - Code analysis and editing agent (submodule in `serena/`)
- **deepl** - Translation services
- **awslabs** - AWS documentation server

### Submodules
- **serena/** - Advanced code analysis and editing agent with LSP support
  - Python-based project with uv package manager
  - Provides semantic code tools for multiple languages
  - Has its own CLAUDE.md with detailed instructions

### JavaScript/TypeScript Setup
- Uses Bun as the JavaScript runtime and package manager
- TypeScript configuration in `tsconfig.json` with strict mode and ESNext target
- Dependencies include MCP servers and Playwright for browser automation

## Key Environment Variables

Required in `.env` (see `.env.example`):
- `GOOGLE_CLOUD_PROJECT` - GCP project ID
- `ANTHROPIC_BASE_URL` - Anthropic API endpoint (optional gateway)
- `BRAVE_API_KEY` - Brave Search API key for MCP server
- `SERENA_PATH` - Path to serena submodule
- `DEEPL_API_KEY` - DeepL translation API key

## Development Workflow

1. **Initial Setup**: Run `./create-symlinks.sh` to link all dotfiles to home directory
2. **Shell**: Fish shell is configured with Homebrew, custom paths, and thefuck alias
3. **Package Management**: Uses Bun for JavaScript/TypeScript packages
4. **MCP Servers**: Configured via `.mcp.json` for AI-enhanced development

## Important Notes

- The repository uses Bun instead of npm/yarn/pnpm for JavaScript package management
- Serena submodule has its own comprehensive development setup - refer to `serena/CLAUDE.md`
- Environment variables must be configured in `.env` based on `.env.example`
- Fish shell configuration sources multiple files for modularity