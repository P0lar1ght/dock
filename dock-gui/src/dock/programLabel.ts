// 命令的主程序，给 bash 卡标题认类型用（规则同 Dock TUI 的 scrollback/execute.rs `program_label`）：
//   cd dock-gui && find src … | wc -l       → find
//   RUST_LOG=debug cargo test -p x          → cargo test
//   git -C repo status                      → git status
// 跳过开头的 cd / export 等铺垫段、环境变量赋值和 sudo / time 之类前缀；
// 管道只看第一段；带子命令的程序（cargo、git、npm…）连子命令一起。认不出给 undefined。

const SETUP = new Set(["cd", "pushd", "popd", "export", "unset", "set", "source", "."]);
const PREFIX = new Set(["sudo", "time", "env", "nohup", "exec", "command"]);
const SUBCOMMANDS = new Set([
  "cargo", "git", "npm", "pnpm", "yarn", "bun", "npx", "go", "docker", "kubectl", "gh",
  "brew", "uv", "pip", "rustup", "deno", "make",
]);
const ENV = /^[A-Za-z_][A-Za-z0-9_]*=/;

export function programLabel(command: string): string | undefined {
  const flat = command.replace(/&&|\|\||\n/g, ";");
  for (const segment of flat.split(";")) {
    const words = (segment.split("|")[0] ?? "")
      .split(/\s+/)
      .map((w) => w.replace(/^['"]+|['"]+$/g, ""))
      .filter(Boolean);
    const at = words.findIndex((w) => !ENV.test(w) && !PREFIX.has(w));
    if (at < 0) continue;
    const program = words[at];
    if (SETUP.has(program)) continue;
    const name = program.split("/").pop() ?? "";
    if (!name) continue;
    if (SUBCOMMANDS.has(name)) {
      let skipValue = false;
      for (const w of words.slice(at + 1)) {
        if (skipValue) {
          skipValue = false;
          continue;
        }
        // `git -C <目录>` / `git -c k=v`：选项带值，值不是子命令。
        if (name === "git" && (w === "-C" || w === "-c")) {
          skipValue = true;
          continue;
        }
        if (w.startsWith("-") || w.startsWith("+")) continue;
        if (w.includes("/") || w.includes("=")) break;
        return `${name} ${w}`;
      }
    }
    return name;
  }
  return undefined;
}
