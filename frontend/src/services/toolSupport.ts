type ToolSupport = { tool_calling: boolean; tool_support_source?: 'runtime' | 'template' | null };

/** How sure the app is that a model can use tools, in plain words. */
export function toolSupportLabel(model: ToolSupport): string {
  if (model.tool_support_source === 'runtime') return model.tool_calling ? 'Tools confirmed' : 'No tool support';
  if (model.tool_calling) return 'Tools declared by template';
  return 'Tools unverified';
}

/** What documents a model can create, when its runtime reported no tool support. */
export function documentKindsNote(model: ToolSupport): string | null {
  if (model.tool_support_source !== 'runtime' || model.tool_calling) return null;
  return 'Documents: plain text only (.txt, .md, .csv, .html, .json). Word, PowerPoint, Excel and PDF files need a model with tool support.';
}
