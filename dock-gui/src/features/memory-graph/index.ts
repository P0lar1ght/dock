// 记忆图谱（Dock `memory/*`）：侧栏多一项「记忆」，点开是笔记的关系图与管理。
// 只在这里往宿主登记，宿主不 import 本目录。

import { BrainCircuit } from "lucide-react";
import { registerNavPage } from "../registry";
import { MemoryPage } from "./MemoryPage";

registerNavPage({ id: "memory", label: "记忆", icon: BrainCircuit, Page: MemoryPage });
