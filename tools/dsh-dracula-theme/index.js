/**
 * Dracula (https://draculatheme.com) palette for the DeepSeek Harness UI.
 *
 * The theme travels as a boot `style` row: the Host renders it into the served
 * index.html and hands the same table to Desktop over its boot IPC, so one
 * definition skins both clients. `html body` out-specifies ui-theme's own
 * `body` / `body[data-ds-dark-theme]` token sheets, which the browser injects
 * after this row, so the palette wins without `!important` (the one exception
 * is the macOS menu fill, whose shipped rule is attribute-scoped).
 *
 * Only tokens are replaced; the light and dark preferences both show Dracula,
 * which is why `color-scheme` is pinned to dark.
 */

const BACKGROUND = '#282a36'
const DARKER = '#21222c'
const CURRENT_LINE = '#44475a'
const FOREGROUND = '#f8f8f2'
const COMMENT = '#6272a4'
const CYAN = '#8be9fd'
const GREEN = '#50fa7b'
const ORANGE = '#ffb86c'
const PINK = '#ff79c6'
const PURPLE = '#bd93f9'
const RED = '#ff5555'
const YELLOW = '#f1fa8c'

const CSS = `
html body,
html body[data-ds-dark-theme] {
  color-scheme: dark;

  --dsw-alias-bg-base: ${BACKGROUND};
  --dsw-alias-bg-document-preview: ${BACKGROUND};
  --dsw-alias-bg-document-selection: color-mix(in srgb, ${PURPLE} 35%, transparent);
  --dsw-alias-bg-layer-1: #2f3140;
  --dsw-alias-bg-layer-2: #343747;
  --dsw-alias-bg-layer-3: ${CURRENT_LINE};
  --dsw-alias-bg-module-platform: ${DARKER};
  --dsw-alias-bg-multi-select: ${CURRENT_LINE};
  --dsw-alias-bg-overlay: ${CURRENT_LINE};
  --dsw-alias-bg-skeleton: rgb(248 248 242 / 8%);
  --dsw-alias-bg-mask-drop: rgb(40 42 54 / 70%);

  --dsw-alias-border-inverted: rgb(248 248 242 / 6%);
  --dsw-alias-border-inverted2: rgb(248 248 242 / 8%);
  --dsw-alias-border-l1: rgb(248 248 242 / 6%);
  --dsw-alias-border-l2-darkmode-thin: rgb(248 248 242 / 6%);
  --dsw-alias-border-l2: rgb(248 248 242 / 10%);
  --dsw-alias-border-l3: rgb(248 248 242 / 14%);
  --dsw-alias-border-l4: rgb(248 248 242 / 18%);

  --dsw-alias-brand-primary: ${PURPLE};
  --dsw-alias-brand-primary-invert: ${FOREGROUND};
  --dsw-alias-brand-primary-new-colorprimary-new-color: ${PURPLE};
  --dsw-alias-brand-text: ${FOREGROUND};
  --dsw-alias-button-primary-fill: ${PURPLE};
  --dsw-alias-button-primary-hover: #caa8ff;
  --dsw-alias-button-primary-dimmed: ${CURRENT_LINE};
  --dsw-alias-button-contrast-fill: ${FOREGROUND};
  --dsw-alias-button-elevated-fill: #343747;
  --dsw-alias-button-floating-fill: #2f3140;
  --dsw-alias-button-floating-hover: ${CURRENT_LINE};
  --dsw-alias-button-ghost-active-border: ${COMMENT};
  --dsw-alias-button-ghost-active-fill: ${CURRENT_LINE};
  --dsw-alias-button-ghost-active-hover: #4e5266;
  --dsw-alias-button-info-fill: ${CYAN};
  --dsw-alias-button-info-hover: #a4f0ff;

  --dsw-alias-label-primary: ${FOREGROUND};
  --dsw-alias-label-primary-bluish: ${FOREGROUND};
  --dsw-alias-label-primary-dimmed: ${FOREGROUND};
  --dsw-alias-label-primary-foreground: ${BACKGROUND};
  --dsw-alias-label-primary-inverted: ${DARKER};
  --dsw-alias-label-secondary: color-mix(in srgb, ${COMMENT} 40%, ${FOREGROUND});
  --dsw-alias-label-tertiary: ${COMMENT};
  --dsw-alias-label-caption: ${COMMENT};
  --dsw-alias-label-dimmed: #4a4d61;
  --dsw-alias-label-shimmer: color-mix(in srgb, ${FOREGROUND} 45%, transparent);
  --dsw-alias-label-deep-diving: ${PURPLE};
  --dsw-alias-label-deep-diving-shimmer: color-mix(in srgb, ${PURPLE} 30%, ${DARKER});
  --dsw-alias-menu-icon: color-mix(in srgb, ${COMMENT} 25%, ${FOREGROUND});
  --dsw-alias-link: ${CYAN};

  --dsw-alias-interactive-bg-hover: rgb(248 248 242 / 8%);
  --dsw-alias-interactive-bg-active: rgb(248 248 242 / 14%);
  --dsw-alias-interactive-bg-hover-solid: #343747;
  --dsw-alias-interactive-bg-hover-accent: color-mix(in srgb, ${PURPLE} 22%, transparent);
  --dsw-alias-interactive-bg-hover-danger: color-mix(in srgb, ${RED} 15%, transparent);

  --dsw-alias-markdown-code-block: ${DARKER};
  --dsw-alias-markdown-code-block-banner: #262833;
  --dsw-alias-markdown-code-segment-selected: ${CURRENT_LINE};
  --dsw-alias-markdown-code-segment-unselected: ${DARKER};
  --dsw-alias-markdown-inline-code: ${CURRENT_LINE};
  --dsw-alias-markdown-citation: #343747;
  --dsw-alias-markdown-placeholder: #2f3140;
  --dsw-alias-markdown-tag: #343747;

  --dsw-alias-scrollbar-bg-l1: ${CURRENT_LINE};
  --dsw-alias-scrollbar-bg-l2: #4e5266;
  --dsw-alias-scrollbar-hover-l1: ${COMMENT};
  --dsw-alias-scrollbar-hover-l2: ${COMMENT};

  --dsw-alias-state-business-primary: ${PURPLE};
  --dsw-alias-state-business-tertiary: color-mix(in srgb, ${PURPLE} 22%, transparent);
  --dsw-alias-state-success-primary: ${GREEN};
  --dsw-alias-state-success-secondary: #69ff94;
  --dsw-alias-state-success-tertiary: color-mix(in srgb, ${GREEN} 16%, transparent);
  --dsw-alias-state-error-primary: ${RED};
  --dsw-alias-state-error-secondary: #ff6e6e;
  --dsw-alias-state-warn-primary: ${ORANGE};
  --dsw-alias-state-warn-secondary: #ffd08a;
  --dsw-alias-state-warn-label: ${ORANGE};
  --dsw-alias-state-warn-tertiary: color-mix(in srgb, ${ORANGE} 16%, transparent);
  --dsw-alias-state-idle-primary: ${COMMENT};

  --dsw-alias-code-diff-added: color-mix(in srgb, ${GREEN} 12%, transparent);
  --dsw-alias-code-diff-deleted: color-mix(in srgb, ${RED} 12%, transparent);
  --dsw-alias-file-diff-added-bg: #2a3a33;
  --dsw-alias-file-diff-added-gutter: #24322c;
  --dsw-alias-file-diff-added-marker: ${GREEN};
  --dsw-alias-file-diff-deleted-bg: #3c2b2f;
  --dsw-alias-file-diff-deleted-gutter: #2f2226;
  --dsw-alias-file-diff-deleted-marker: ${RED};

  --dsw-alias-illustration-background: #2f3140;
  --dsw-alias-illustration-backing: ${CURRENT_LINE};
  --dsw-alias-illustration-detail: ${COMMENT};
  --dsw-alias-illustration-inset: #343747;
  --dsw-alias-illustration-shadow: #343747;
  --dsw-alias-illustration-surface: #343747;

  --dsw-alias-switch-thumb: ${FOREGROUND};
  --dsw-alias-toast-bg: ${CURRENT_LINE};
  --dsw-alias-toast-label: ${FOREGROUND};
  --dsw-alias-tooltip-bg: ${CURRENT_LINE};
  --dsw-alias-tooltip-key-bg: color-mix(in srgb, ${CURRENT_LINE}, white 18%);
  --dsw-alias-turn-trigger-bg: ${DARKER};
  --dsw-alias-turn-trigger-bg-hover: rgb(248 248 242 / 8%);

  --dsw-specific-bubble: ${CURRENT_LINE};
  --dsw-specific-bubble-highlight: ${COMMENT};
  --dsw-specific-input-major: #2f3140;
  --dsw-specific-login-input: ${DARKER};
  --dsw-specific-selector: #343747;
  --dsw-specific-sidebar-fill: ${DARKER};
  --dsw-specific-sidebar-nav-item-active: ${CURRENT_LINE};
  --dsw-specific-sidebar-nav-item-active-accent: color-mix(in srgb, ${PURPLE} 22%, transparent);
  --dsw-specific-tip: #343747;

  --dsw-menu-surface-fill: rgb(40 42 54 / 72%);
  --dsw-alias-menu-group-header-fill: rgb(33 34 44 / 94%);
  --dsw-specific-menu: rgb(40 42 54 / 94%);

  --shiki-foreground: ${FOREGROUND};
  --shiki-background: ${DARKER};
  --shiki-token-constant: ${PURPLE};
  --shiki-token-string: ${YELLOW};
  --shiki-token-comment: ${COMMENT};
  --shiki-token-keyword: ${PINK};
  --shiki-token-parameter: ${ORANGE};
  --shiki-token-function: ${GREEN};
  --shiki-token-string-expression: ${CYAN};
  --shiki-token-punctuation: ${FOREGROUND};
  --shiki-token-link: ${CYAN};
}

/* The shipped macOS menu rule is attribute-scoped, so it out-specifies the
 * palette above; keep menus opaque enough to read over vibrancy. */
html[data-platform='darwin'] body {
  --dsw-specific-menu: rgb(40 42 54 / 94%) !important;
}
`

/** Cordis plugin name used by the profile entries that load this package. */
export const name = 'dsh-dracula-theme'

/**
 * Contribute the palette as a boot style row.
 * @param ctx - Host context; the row reaches the served index and Desktop's boot payload.
 */
export function apply(ctx) {
  ctx.on('webserver/index-inject', (table) => {
    table.push({ kind: 'style', text: CSS })
  })
}
