/** Avoid accidentally submitting broad/absolute paths. Runtime validates each literal path. */
export const checkpointPaths = (input: string): string[] => [...new Set(input.split(/\r?\n/).map(v => v.trim()).filter(Boolean))];
export const invalidCheckpointPaths = (paths: string[]): boolean => paths.some(path => /^[~/\\]|^[A-Za-z]:|[\x00-\x1f]/.test(path)
  || path.split(/[\\/]/).some(part => part === "." || part === ".." || part === ".git" || !part));
