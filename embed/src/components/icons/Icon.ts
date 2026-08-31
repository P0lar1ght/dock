import { html, svg } from 'lit';

interface IconSource {
  icon: [number, number, string[], string, string | string[]];
}

export function iconTemplate(definition: IconSource, className = 'control-icon') {
  const [width, height, , , pathData] = definition.icon;
  const paths = Array.isArray(pathData) ? pathData : [pathData];
  return html`
    <svg
      class=${className}
      viewBox=${`0 0 ${width} ${height}`}
      aria-hidden="true"
      focusable="false"
      role="img"
    >
      ${paths.map((path) => svg`<path fill="currentColor" d=${path}></path>`)}
    </svg>
  `;
}
