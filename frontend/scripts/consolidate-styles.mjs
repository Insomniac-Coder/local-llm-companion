// One-time mechanical migration: remove superseded shell rules, retaining
// feature-specific styles. The shell now has one owner, workbench.css.
import fs from 'node:fs';
import postcss from 'postcss';
const shellClass = /\.(?:shell|center|sidebar(?:-[\w-]+)?|brand(?:line|-[\w-]+)?|rail(?:-hide|-only)?|primary-nav|utility-nav|nav-glyph|convlist|convrow|convbtn|new-session|workspace-switcher|open-project|topbar(?:-[\w-]+)?|respill|mobile-menu|chat|msg(?:-[\w-]+)?|message-row|composer(?:-[\w-]+)?|send-button|stop-button|toggle(?:-dot)?|empty|rightpanel(?:-[\w-]+)?|codeheader|contextbar|card|setgrid|layout)(?![\w-])/;
for (const file of ['src/styles.css', 'src/ui/ui.css']) {
  const css = postcss.parse(fs.readFileSync(file, 'utf8'));
  css.walkRules((rule) => {
    if (rule.parent.type === 'atrule' && /keyframes/.test(rule.parent.name)) return;
    const retained = rule.selectors.filter((selector) => selector.trim() && !shellClass.test(selector) && !/^(?::root|\[data-theme=|body\b|button\b|input\b|select\b|textarea\b|\*$)/.test(selector));
    if (!retained.length) rule.remove();
    else rule.selectors = retained;
  });
  css.walkAtRules((rule) => { if (rule.nodes && !rule.nodes.length) rule.remove(); });
  css.walkComments((comment) => comment.remove());
  fs.writeFileSync(file, css.toString().replace(/\n[\t ]*\n(?:[\t ]*\n)+/g, '\n\n').trim() + '\n');
}
