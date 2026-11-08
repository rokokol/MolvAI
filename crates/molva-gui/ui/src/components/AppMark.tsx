// SPDX-License-Identifier: MIT
// The mark of the window: the site's pixel logo, drawn on its own 22-unit grid so one unit is
// one pixel at any size. Every fill is a role of the deliberate palette the kit ships, taken by
// name rather than typed here, so the mark stays the same on either theme

export default function AppMark() {
  return (
    <span className="app-mark" aria-hidden="true">
      <svg viewBox="0 0 22 22" role="presentation" focusable="false">
        <g shapeRendering="crispEdges">
          <rect width="22" height="22" fill="var(--ddlc-yuri-shadow)" />
          <rect width="22" height="2" fill="var(--ddlc-pink)" />
          <rect x="3" y="4" width="16" height="12" fill="var(--ddlc-dot)" />
          <rect x="2" y="5" width="18" height="10" fill="var(--ddlc-dot)" />
          <rect x="3" y="5" width="16" height="1" fill="var(--ddlc-paper)" />
          <rect x="5" y="16" width="6" height="1" fill="var(--ddlc-dot)" />
          <rect x="6" y="17" width="4" height="1" fill="var(--ddlc-dot)" />
          <rect x="7" y="18" width="2" height="1" fill="var(--ddlc-dot)" />
          <g fill="var(--ddlc-yuri)">
            <rect x="4" y="8" width="2" height="4" />
            <rect x="7" y="6" width="2" height="8" />
            <rect x="10" y="6" width="2" height="9" />
            <rect x="13" y="7" width="2" height="6" />
            <rect x="16" y="8" width="2" height="3" />
          </g>
        </g>
      </svg>
    </span>
  );
}
