// 记忆图谱（Dock `memory/*`）：侧栏多一项「记忆」，点开是笔记的关系图与管理。
// 只在这里往宿主登记，宿主不 import 本目录。

import { BrainCircuit } from "lucide-react";
import { deferred } from "../deferred";
import { registerNavPage, type NavPageProps } from "../registry";

// 整页连同图谱引擎（d3-force）按需加载：启动路径上不带，第一次点开「记忆」才取。
const page = deferred<NavPageProps>(() => import("./MemoryPage").then((m) => m.MemoryPage));

registerNavPage({ id: "memory", label: "记忆", icon: BrainCircuit, Page: page.Viewer });
