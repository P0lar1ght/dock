# Rhai 坑位（按 Dock 用的 rhai 1.25 实测）

Rhai 长得像 JS / Rust，但不是。下面每条左边是会报错的写法。

## 语法

| 报错 / 现象 | 错的写法 | 对的写法 |
| --- | --- | --- |
| `Unexpected 'try'` | `let x = try { … } catch (e) { … };` | `try` 只能当语句：`let x = (); try { x = f(); } catch (e) { x = "失败"; }` |
| `Function definitions must be at global level` | 在 `apply` / 闭包 / 块里写 `fn g() {}` | `fn` 写在文件顶层，或改成闭包 `let g = \|a\| { … }; g.call(1)` |
| `Variable not found` 出现在顶层 `fn` 里 | 顶层 `fn` 用外面的 `host` / `store` | 顶层 `fn` 不捕获变量：当参数传进去 `fn fetch(host, q) { … }` |
| `Expecting ';' to terminate this statement` | 两条语句之间漏分号 | 除块里最后一个表达式外，每条语句以 `;` 结尾 |
| 同上，出在 map 字面量 | `{ a: 1 }` | map 字面量是 `#{ a: 1 }`；`{ }` 是语句块 |
| `'null' is a reserved keyword` | `null`（也没有 JS 那套空值） | 空值是 `()`，判断用 `x == ()` |

## 函数

| 报错 | 原因 | 用什么 |
| --- | --- | --- |
| `Function not found: is_string` | 没有 `is_string` / `is_map` 这类函数 | `type_of(x) == "string"`；其它类型名：`i64` `f64` `bool` `map` `array` `()` `Fn` `blob` |
| `Function not found: try_cast` / `key_of` … | Rust API 名，不是脚本函数 | 按类型分支用 `type_of`；遍历 map 用 `for k in m.keys() { … }` |
| `Function not found: xxx (())` | 参数是 `()`：上一步取了不存在的字段 | 先兜底：`args.q ?? ""`，或 `if x == () { … }` |

不确定某个函数在不在，查 `cordis_inspect what: "builtins"`（Dock 加的 host 与脚本函数），
其余是 Rhai 标准库。

## 好用、而且确实能用的

- 字符串拼数字：`"HTTP " + 404`、`404 + " 次"` 都行。
- 模板字符串：`` `共 ${n} 条` ``。
- 空值兜底：`m.x ?? "默认"`；安全取字段：`m?.x`。
- 取不存在的 map 键得到 `()`，不抛错。判断键在不在：`"a" in m` 或 `m.contains("a")`。
- 闭包按引用共享外层变量：在 `apply` 里 `let store = #{…}`，各个 `execute` 改的是同一份。
- `return` 可以在闭包里提前返回。
- `switch x { 1 => "a", _ => "b" }`、`for i in 0..n { }`、`s.split(",")`、`parse_int("42")`、`x.to_string()`。

## 限制

- 操作数有上限，死循环会被掐断；拦截钩子里尤其要短。
- 没有 `eval`。
- 字符串最长 1 MiB，map 最多 16384 个键。
