# TsinghuaKit 公共 API 与工程组织重构方案

状态：重构实施中。当前明确采用普通 JSON 文件：默认只在内存；用户逐项启用后，TsinghuaKit 将凭据、Identity 会话快照和 NetworkProfile 写入宿主选择的目录。文件不加密，也不调用 Keychain、Keystore、Flutter Secure Storage 或其它系统凭据存储；Unix 下目录/文件权限为 `0700`/`0600`。读取该文件的同一 OS 用户进程仍可直接取得内容。Identity 与 SelfService 账号域分开保存，记住密码默认关闭。较早章节中的加密文件方案属于历史实现或已废弃提案，以本段与第 50 节的当前决定为准。THYou App 仍在迁移旧 Runtime，具体状态以本节的 SDK/App 集成记录为准；Portal/Tsinghua Secure 是本机网络输入/操作，不是第三个账号。App 的 72 个旧 gateway 方法逐项映射见 [Flutter API 迁移矩阵](flutter-api-migration-matrix.md)。

Cargo workspace 已拆成 `tsinghua_kit` 公共 SDK、`tsinghua_kit_engine` 内部协议实现和 `tsinghua_kit_ffi` 兼容桥接包。SDK 已不依赖 Flutter 或 FRB，并通过 engine 持有实际 Rust Runtime；FRB 生成代码仍留在定义兼容 DTO 的 FFI crate 中，避免只 re-export 远端类型而触发 orphan-rule 编译错误。当前工作树的 SDK 提供两个 Auth 账号域、网上服务大厅待办、SelfService 账号/设备/用量只读查询、INFO 新闻读取、Learn 课程/公告/作业/资料/讨论读取及资料保存、Registrar 课表/成绩/考试、校历/学期时间线、图书馆场馆/楼层/分区/时段/座位/插座只读查询、教室楼栋和周状态，以及校园卡账户和完整有界交易读取、电费余额与缴费记录读取。校园卡目标密码按一次性服务交互处理，仍属于统一身份派生 handoff，不是第三个账号域。公开 API 仍是逐批推进的垂直切片，不代表所有服务、Flutter 包或 App 已迁移。

Flutter facade 继续保留 `tsinghua_kit.dart` 作为便捷总入口，并新增 `core.dart` 与 `auth.dart`、`network.dart`、`service_hall.dart`、`self_service.dart`、`news.dart`、`learn.dart`、`registrar_calendar.dart`、`library.dart`、`classrooms.dart`、`campus_card.dart`、`electricity.dart` 和 `read.dart` 领域入口。`core.dart` 放初始化、Client 创建和共享错误；其余入口只重导出对应公开 DTO 与 Client，不另建 Runtime，也不导出生成的 FRB 模块。`AuthStatus` 分别携带两个账号域的可选 username；Rust 调试输出仍脱敏。消费者可按领域导入，所有 facade handle 仍由一个 `TsinghuaKitClient` 持有。

Rust crate 也按单一入口策略收口：根路径只重导出 `Client`、`ClientBuilder`、`Error` 与 `Result`；缓存、凭据、Identity 快照和 NetworkProfile 持久化策略归入 `config`，服务 DTO、查询策略、引用和领域 Client 只从对应的 `auth`、`network`、`service_hall`、`self_service`、`news`、`learn`、`registrar`、`calendar`、`library`、`classrooms`、`campus_card`、`electricity` 与 `read` 模块导入。Flutter FFI crate 按新的路径导入配置，但 bridge DTO 和 FRB 生成绑定不暴露给普通 Rust 消费者。

当前实现进度：外部 Rust 门面提供 `Client`、`ClientBuilder`、`auth().identity()`、`auth().self_service()`、双账号状态、结构化错误、`ReadResult`、service-hall 待办/目录/已办、草稿、抄送、阶段事项与阶段详情、SelfService 只读查询、INFO 新闻目录/列表/搜索/详情/收藏/订阅及订阅文章页，以及 Learn 课程目录、课程公告、作业列表/详情、课程资料/分类、讨论和资料保存，以及图书馆场馆、楼层、分区、开放时段、座位和插座只读查询、教室楼栋和周状态读取，以及校园卡账户和完整有界交易读取、来源感知的电费余额与缴费记录读取。设备断开通过不透明 `DeviceRef` 显式选择，新闻详情通过不透明 `ArticleRef` 选择；来源与栏目筛选通过不透明目录引用选择，订阅文章通过不透明 `NewsSubscriptionRef` 选择。Learn 的 `CourseRef`、`HomeworkRef` 和 `CourseFileRef` 绑定创建它们的 Client 与当前目录代次；作业和资料引用另有五分钟本地时效，过期或跨 Client 使用在认证 handoff 和 HTTP 之前被拒绝，资料保存还会由 Runtime 重读目录并绑定 Identity 账号和 Cookie jar。校园卡流水范围只接受不超过 31 天的日期，所有分页完整成功后才返回；流水 ID 留在 Rust 内部去重，账户/交易结果保留真实 live/cache 来源、新鲜度与观测时间。首次卡片 handoff 若要目标密码，SDK 返回稳定 `InteractionRequired` 并提供当前 Client 的 `PasswordRequired` 提示；密码挑战限时、绑定 Identity 用户及固定卡片来源，一次性消费后才提交，临时密码输入和 wire 密文均零化。卡片二次认证从 `auth().identity().interaction()` 查询方法并通过显式 code flow 完成，不自动重登或重放原读取。Registrar facade 提供来源感知的整学期课表、成绩和完整考试报告；校历 facade 提供 Learn 当前/后续学期时间线及类型化选择的学校校历图片。课表、成绩和考试缓存来源及新鲜度保留在 `ReadResult`，研究生考试区间按原始标签呈现；学期时间线日期、周一对齐规则和图片响应在 SDK 映射边界再次验证。课程目录与公告保留 Runtime 验证的实时/缓存来源、新鲜度和观测时间；作业、讨论、资料与分类明确只从实时接口读取。讨论和资料目录到 200 条上限时用 `ReadCoverage::Partial(ReadLimitReached)` 表达，不伪装为完整；保存资料要求调用者明确提供目标路径，Runtime 检查路径并拒绝覆盖既有文件。新闻分页和搜索输入经过校验，缓存读取策略及来源、新鲜度、观测时间保留在结构化结果中；新闻目录明确报告栏目集合完整或不完整，收藏集合只有完整分页后才返回。校方 URL、文章/课程/作业/文件 ID、筛选器 ID、各类 selector、订阅关键词、错误文本、公告/讨论/资料正文、HTML 和作业答案正文不会出现在引用或结果的 `Debug` 输出中。兼容 FFI 的下标操作仍留在内部。门面将 engine 的 `CampusRuntime` 与借用型服务实现包在 SDK crate 内，外部 Rust 消费者不直接依赖这些实现类型。一个 Client 只持有一个 Runtime，登录与验证码经同一个受控 transport 串行执行；默认会话仅驻留内存，临时缓存目录随 Client 释放。THOS 保留首页计数、退回事项合并、完整分页和 60 秒账号绑定缓存；SDK facade 统一暴露服务目录、四类类型化列表和由完整阶段列表产生的短期详情引用。SelfService 业务结果绑定第二个账号槽；仅迁入从当前 DeviceRef 明确选择的设备断开，其它写操作不公开。网络 facade 现支持 Portal/EAP 资料 CRUD、显式密码填入、memory-only 和宿主选择目录的普通 JSON 文件。Identity cookie 快照也是独立的 JSON 目录选项。新的 Flutter `0.2.0-alpha.1` facade 已桥接双账号 Auth 登录/状态、Identity 与 SelfService 单域退出、显式全部退出、本机网络资料 CRUD、显式表单准备和显式密码填入；Identity 单域退出会撤销派生服务证明，并在尚有共享通道依赖时保留 SelfService 选择为 `Expired`。旧兼容 Runtime 的全局 logout 语义仍不变。未知二次认证方式保留为 `unknown`，在提交前明确拒绝，不映射到其它方式。macOS 临时消费者已通过 FFI 初始化、双账号状态、分域退出、Rust 错误到公开异常的映射，以及 service-hall、SelfService、Registrar、Calendar、INFO 和 Learn 的无会话/无效输入门禁。service-hall 现已桥接 pending、完整服务目录、四种任务视图与 Client 绑定的阶段详情；SelfService 已桥接账号/设备/用量和不透明 DeviceRef 断开；Registrar 已桥接课表、成绩、考试，Calendar 已桥接 Learn 学期和学校校历图片，INFO 已桥接目录、分页、搜索、详情、收藏和订阅读取；Learn 已桥接课程、公告、作业、作业详情、资料、分类、讨论和显式路径保存。仍待实现：SelfService 跨进程会话恢复、SystemWifiEap 系统配置和其它上下文句柄；Auth credential vault 按逐次 opt-in 写入宿主目录中的普通 JSON，不接入 Keychain/Keystore。Portal connector 已接入 Rust 与 Flutter facade。Library、Classroom、CampusCard、Electricity 的首批 Dart/FRB facade 已接入，但 THYou App 仍锁定旧 `v0.1.1` bridge，尚未迁移到新 facade。更多 THOS 写入能力及能力目录仍待迁入。TUNet 不进入账号恢复链，仍作为本地网络操作处理。

Learn facade 还包括课程资料、资料分类、讨论列表和显式资料保存。资料/讨论列表保留最多 200 条的服务限制；确认达到边界时结果成功但覆盖状态为 `Partial(ReadLimitReached)`。资料保存引用来自最近一次列表，限定 Client、课程目录代次和五分钟时效，并在 Runtime 中按固定 Learn 路径重新核对文件后才保存；调用者必须提供目标路径，既有文件不会被覆盖。讨论时间仍以学校返回的显示标签呈现，不对没有时区证据的字符串擅自转换为 UTC。`courses()` 与 `announcements()` 沿用 Runtime 已有的 fresh-cache 优先与 stale fallback 行为，公共调用者暂时不能指定 `ReadPolicy`；作业、资料、讨论与详情没有缓存，都是显式实时读取。旧 Runtime 这几条入口仍返回 `String`，因此 facade 只会根据 Identity 状态报告登录/交互错误，其他失败使用明确但较粗的 `ServiceUnavailable`；不可在 SDK 层匹配中文错误来伪造网络、限流、会话过期等细分类别。学期时间线和图片校历已进入独立 `calendar()` facade，但旧 Runtime 错误分类仍较粗；下一步应将错误分类移动到协议错误产生位置，并继续迁移其余旧 `String` 接口。

校园网领域契约已明确区分 `Portal` 与 `SystemWifiEap` 两种本机资料用途。Rust facade 可在 owning Client 生命周期内保存资料及用户显式选择保存在内存中的密码、准备不含密码的表单数据；还可通过同一 Client 生成的当前版本 `PreparedNetworkInput` 明确发起 Portal 连接，并仅断开当前进程内经验证的一次性目标。密码覆盖只用于这一次 Portal 操作，不持久化；省略覆盖值时 Rust 只读取资料中用户已显式保存的密码。`SystemWifiEap` 在任何 Portal 请求之前被拒绝。当前默认不持久化；显式选择后，Rust 使用 `NetworkProfileStoragePolicy::JsonDirectory` 在宿主选择的目录保存可读 JSON；不使用平台安全存储。显式表单填入仍需通过匹配当前版本的 `PreparedNetworkInput` 取得脱敏、非 `Clone` 的 `NetworkProfilePassword`。Tsinghua Secure 的操作系统 802.1X 配置仍未实现，必须按平台能力和 OS 授权返回明确结果；资料保存/填入不能显示为已连接。

仍未完成的主要工作：更多 THOS/能力目录、SelfService 跨进程会话恢复、Flutter App 生产迁移和操作系统 Wi-Fi/EAP 适配入口；Auth 凭据 vault、网络 Profile 与 Identity 快照均由 SDK 管理应用私有文件；不依赖 Keychain/Keystore。当前 Learn 作业详情是只读文本和附件元数据，不提供提交、下载或预览；讨论只公开列表，不公开帖子详情或发帖；课程资料保存不提供进程内预览，也不会覆盖已有路径。SelfService 目前提供账号摘要、在线设备查询、用量/余额及基于最新列表 `DeviceRef` 的显式断开，也可在明确选择保存后通过 Rust 启动新的图片验证码流程；它不恢复会话或绕过验证码。真实 Keychain/Keystore 集成运行验证、Tsinghua Secure 系统适配器、FRB/Flutter 包独立发布与 App 消费切换仍未完成。`SystemWifiEap` 不能代表已能写入操作系统配置。现有 FFI 包仍保留旧 `CampusRuntime` 兼容入口。校园卡 facade 仍使用 legacy Runtime `String` 失败，所以除当前 Identity/交互状态外其他错误只归为稳定 `ServiceUnavailable`；尚未进行真实账号访问。

已知验证证据：此前 crate 初拆时的 workspace check、doc、FFI artifact 和 Cargokit 测试记录仍按原报告保留。SDK 外部集成测试有 6 项通过；engine 中 `backend_refactor_` 定向测试有 9 项通过。2026-09-25 又验证了带 FRB 生成模块的当前 FFI library check/build、公共 SDK Rustdoc、格式和补丁空白。engine 仍报告一批原兼容实现已有的 unused/dead-code warning；没有运行全部约 1,600 项 engine 单测或线上账号验证。当前环境中的 Cargokit Dart 定向测试未能加载：Flutter 所带 Dart SDK 缺少 `frontend_server.dart.snapshot`，因此不能记为通过。此次发现并保留了 FRB 生成 DTO 必须由定义它们的 crate 实现桥接 trait 的边界，没有让 App 绑定退化为非法的跨 crate 实现。

发行边界：SDK 目前依赖同一仓库中的 `tsinghua_kit_engine` 路径 crate。`cargo package --locked --allow-dirty -p tsinghua_kit` 在 crates.io 依赖解析处失败，因为 engine crate 尚未发布。GitHub 公共仓库消费与 crates.io 发布是两种交付方式；后续若要发布 registry 包，需先发布 engine 并按依赖顺序发布 SDK，或将内部 engine 源码收回 SDK 包。本轮没有发布 crate，也没有完成从干净外部 Git 消费者目录进行安装验证。

账号模型已根据后续需求细化：Auth 仅管理统一身份与网络自助两个可选账号；校园网连接属于本机操作，其资料可以保存/自动填写，但不作为第三个持久认证账号。详细约定见 [双账号与本地网络连接设计](auth-account-network-design.md)。

## 1. 建议采用的方向

把 TsinghuaKit 定义为一个以 Rust 为核心的校园服务 SDK。外部应用通过一个 Client 管理两类可选账号，按服务调用业务方法；本机校园网连接具有独立的资料与操作入口。结果与交互认证流程采用结构化表达。

仓库最终有三个清楚的消费边界：

1. **Rust SDK `tsinghua_kit`**：公开客户端、两个认证域的账号模型、业务查询、本地连接资料/操作、交互和错误；HTTP、Cookie、ticket、CSRF、解析器、恢复账本、缓存格式留在私有实现中。
2. **Flutter 包 `tsinghua_kit` 与 Rust FFI crate `tsinghua_kit_ffi`**：适配 Rust SDK，维护一起生成的 Dart/Rust 桥接代码。Flutter 使用者直接 import 包的公共入口。
3. **CLI `tsinghua-kit-check`**：用公共 SDK 做只读验收，负责终端交互、用例选择、调度和脱敏报告。

首先收敛公共契约、拆出共享实现，再移动 Cargo package 和 Flutter 目录。现有校方协议、严格解析、账号绑定和认证恢复逻辑保留并逐段迁移。业务服务在一个 SDK crate 内按模块划分，当前没有必要把每个学校系统都变成独立 crate。

## 2. 当前问题及证据

### 2.1 公共入口暴露了实现结构

当前 `lib.rs` 有 **41 个 `pub mod`、43 条 `pub use`**，其中包括 `domain::*`、`protocol::*` 通配导出。现有默认 feature 的 Rustdoc 首页按链接去重后有 **425 个条目链接**：41 个模块、219 个 struct、102 个 enum、40 个函数、16 个常量、5 个 type、2 个 trait。这是首页可见条目统计，不是完整 SemVer API 检查。

业务入口旁边同时出现 `UrlEncodedSecondAuthForm`、`BoundCsrfToken`、`SessionRegistry`、`RegistrarCalendarRequestPlan`、`MultipartBoundaryPlan`、`JsonFileCache`、HTML 解析函数等实现类型。使用者难以判断应该调用 `CampusRuntime`，还是自己组合 `IdentityClient`、`LearnClient` 和会话协调器。

`CampusHttpTransport` 还公开了原始 `reqwest::Client` 和 Cookie jar 的 getter。外部使用者因此可以直接依赖这些底层对象，SDK 很难把“所有实际请求走统一门禁”作为公共调用路径的保证。这里说明的是 API 设计暴露面，不是认定当前 App 已经绕过门禁或泄露凭据。

证据：[重构前的 crate 导出](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/lib.rs)、[重构前的 transport](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/transport.rs#L489)。

### 2.2 一个应用运行时承载了过多职责

`api/runtime.rs` 共 **25,251 行，包含内嵌测试**；主测试模块从第 18,240 行开始。`CampusRuntime` 有 **76 个公开方法、85 个直接状态字段**；同一文件定义了 **49 个公开 DTO struct**。

其职责横跨统一认证、WebVPN、二次认证、所有服务读取、验证码交互、私有存储、失败恢复、数据映射、页面偏好、业务目录、网络断开以及终端验证。`runtime_thos.rs` 等拆分文件通过 `use super::*`、`&mut CampusRuntime` 继续访问整个运行时，拆文件后依赖边界仍然很宽。

不能只用行数判断质量：这里包含很多真实问题的修复与回归证据。需要拆分的是状态所有权和依赖方向，而不是为了缩短文件删除保护逻辑。

证据：[基线运行时](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/runtime.rs#L2423)、[基线 THOS 子模块](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/runtime_thos.rs)。当前实现位于 [engine runtime](../rust/crates/tsinghua-kit-engine/src/api/runtime.rs)。

### 2.3 Rust SDK 与 Flutter 桥接没有独立边界

`flutter_rust_bridge` 是当前 crate 的必选依赖；同一个 crate 同时输出 `rlib`、`staticlib`、`cdylib`。多数高层操作返回 Flutter 风格的 DTO 和 `Result<_, String>`。

此外，App 已生成的 `lib/src/rust/api/service_catalog.dart` 中出现 `RuntimeServiceProofs`、`RuntimeCacheCapabilities` 及其 `default_()`，而对应 Rust 类型声明为 `pub(crate)`。这些不是原始认证凭据，但说明仅靠 Rust 可见性和扫描 `crate::api` 不能完整定义桥接面，需要对实际生成的 Dart API 做独立检查。

当前 Dart 绑定由消费 App 保管，库升级需要 App 自行找到源码、对齐生成器版本并再次 codegen。桥接协议的一致性责任因此落到了每个消费者身上。

证据：[Cargo 清单](../rust/Cargo.toml)、[基线服务目录类型](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/service_catalog.rs)。App 侧基线位于 [FRB 配置](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/flutter_rust_bridge.yaml)、[源码扫描脚本](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/tools/tsinghua_kit_source.py) 和生成的服务目录 binding。

### 2.4 公共契约以字符串和重复 DTO 为主

典型例子：

- `login` 接收 `graduate: Option<bool>` 及三个额外布尔参数；调用者需要知道参数之间的关系。
- `load_thos_task_list(kind: String, ...)`、`establish_service_session(service: String)` 接受运行时才验证的选择值。
- `CampusRuntimeStatusDto.state`、数据 `source/status`、二次认证方式等是字符串。
- `load_info_news_detail` 与 `load_info_news_detail_result`、多个普通读取与 `_result` 读取并存；后者补来源信息，前者遗留兼容。
- 列表有时返回 `Vec<T>`，有时返回 `empty`、`generated_at`、`source`、`status`、`error`；部分字段为了兼容旧 gateway 是 `Option`。
- `load_classroom_state` 使用列表下标，`disconnect_usereg_device` 也使用列表下标；下标本身没有表达目录版本和账号归属。

App 的 `lib/data/campus_runtime.dart` 因而有大量字符串状态校验，以及基于错误文本进行展示分类的逻辑。边界校验应保留，但错误的业务类别应由 Rust 用结构化类型明确传递。

证据：[基线登录入口](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/runtime.rs#L3634)、[基线 THOS DTO](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/thos.rs)、[基线新闻 DTO](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/runtime.rs#L1073)。

### 2.5 库仍默认绑定具体 App

`create_runtime` 调用 `init_app_logging`；后者尝试注册进程级 tracing subscriber，并选择 THYou 日志目录。持久化路径在 macOS 上直接使用 `com.thyou.thyou` 容器，在其他平台也使用 THYou 名称。

`cache_path` 表面可配置，但生产入口必须位于固定应用私有根目录下。这一限制本身有保护作用；问题是外部宿主无法声明自己的合法私有根目录。

`ExperiencePreferencesDto` 内有首页区块顺序、隐藏区块、常用服务等页面偏好，服务目录还带 `icon_key`、本地化 key 和默认可见性。它们不应成为通用校园 SDK 的业务承诺。

证据：[基线日志初始化](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/telemetry.rs#L795)、[基线应用目录](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/session_persistence.rs#L475)、[基线缓存路径校验](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/runtime.rs#L15760)、[基线页面偏好](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/experience.rs)。

### 2.6 测试、兼容层和 CLI 影响了公共 API

`InMemoryCampusService` 等兼容类型仍然公开，但生产构建下操作会拒绝执行；`api::load_overview` 是始终返回错误的旧入口。默认文档出现这样的名字，会误导新使用者。

`rust/tests/*_contract.rs` 又直接导入大量底层 client、解析器和会话类型。如果不先搬迁内部协议测试，收紧可见性就会破坏这些测试，容易反过来促使实现细节继续公开。

终端验收在 `api::runtime::cli_validation` 中，通过 `use super::*` 使用运行时私有状态；调试验收也有构造函数中的触发逻辑。CLI 尚未形成独立的 SDK 使用者边界。

证据：[基线兼容 fixture](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/services.rs#L101)、[基线概览入口](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/campus.rs#L553)、[基线验收调度](https://github.com/canxin121/TsinghuaKit/blob/a4db9fbe1c1480bcbaa3bab203d1bd9e37556621/rust/src/api/cli_validation.rs)、[当前公共 API 测试](../rust/tests/public_api.rs)。

## 3. 重构后允许公开什么

| API 层 | 公开内容 | 应当保留在内部的内容 |
| --- | --- | --- |
| 客户端 | `Client`、`ClientBuilder`、宿主配置、双账号状态 | Cookie jar、reqwest client、认证恢复账本 |
| 认证 | `LoginRequest`、`AuthStep`、挑战句柄、认证方式、服务状态 | ticket、CSRF、登录表单、SM2 密文、WebVPN 导航 |
| 业务服务 | 课程、成绩、新闻、网上服务大厅、图书馆、校园卡等服务方法 | 校方 URL、请求计划、HTML selector、解析器、适配器配置 |
| 输入 | 学期/学段选择、分页查询、日期范围、经过验证的资源引用 | 自由拼接的路径、任意来源 URL、认证协议字段 |
| 输出 | 业务模型、来源和新鲜度、分页完成度、稳定错误类别 | 原始 HTML 响应、原始响应头、持久化 envelope 格式 |
| 宿主集成 | 私有存储配置、显式诊断订阅、限于必要范围的取消/关闭接口 | App 图标 key、首页布局、终端报告写盘 |
| 本地校园网连接 | Portal 与系统 EAP 分用途的可选本机资料、自动填写摘要、受平台能力约束的显式连接/断开、当前网络观测 | 临时 challenge、密码、平台授权与连接操作校验；不创建 Auth 账号会话 |

默认公共模块建议如下。根目录只 re-export 少量常用入口和错误，具体业务类型从所属模块导入：

```rust
pub use client::{Client, ClientBuilder};
pub use error::{Error, ErrorKind, Result};

pub mod auth;
pub mod config;
pub mod read;
pub mod learn;
pub mod registrar;
pub mod calendar;
pub mod news;
pub mod service_hall;
pub mod library;
pub mod classrooms;
pub mod campus_card;
pub mod electricity;
pub mod self_service;
pub mod network;

mod client;
mod error;
mod internal;
```

`internal` 中包含 transport、会话协调、恢复规则、存储、遥测和各业务协议。跨内部模块使用 `pub(crate)` 或更窄的可见性。公开 API 不 re-export `internal`，不提供一个可以绕过这些约束的通用 raw client。

## 4. 外部 Rust 使用方式

### 4.1 一个 Client，按业务借用服务入口

拟议调用形态：

```rust
use tsinghua_kit::{Client, config::StoragePolicy, read::ReadPolicy};

let mut client = Client::builder()
    .storage(StoragePolicy::MemoryOnly)
    .build()?;

// 统一身份通过 client.auth().identity() 交互完成。
// 以下读取发生在相关认证流程完成后。
let pending = client.service_hall()
    .pending(ReadPolicy::Refresh)
    .await?;

let courses = client.learn()
    .courses(course_query, ReadPolicy::PreferFreshCache)
    .await?;
```

`client.learn()` 等是轻量借用视图，不会创建新的运行时。一个 Client 内有统一身份和网络自助两个账号作用域，username 可以不同；共享受控 transport 不等于共享账号。首先保留单一所有者和 `&mut` 异步方法的语义：读取可能触发服务 handoff，本来就需要管理可变状态。

Flutter/CLI 的有界任务队列继续进入同一客户端。内部已有独立数据源并发可继续存在。此阶段不承诺外部能同时借用多个可变服务句柄，也不为了表面上支持 `Clone` 而引入多份会话。

以后如果明确需要并发 Rust 调用，可以评估一个单一所有者的命令队列；这属于单独的性能和取消语义设计，不与第一轮接口重构一起完成。

### 4.2 构造与会话恢复分开

`Client::builder().build()` 负责验证配置和建立本地对象，不访问校方接口，不自动登录，不抢占全局日志 subscriber。内存存储是默认方案。

需要持久化的宿主显式传入应用身份和私有目录配置。配置只决定合法存储边界，目录规范化、符号链接检查、账号绑定、加密和文件权限仍由 Rust 执行。

本地快照加载与在线会话证明必须有不同入口和状态：

- 本地恢复允许显示所属认证域与账号匹配的已验证缓存。
- 只有在线服务证明成功才能把对应服务标记为在线可用。
- `client.auth().identity().resume()` 与 `client.auth().self_service().resume()` 分别执行有界恢复；网络自助仍受实际访问路径前置条件约束。
- 校园网保存资料只用于自动填写/显式连接，不进入上述恢复流程。drop 或账号注销不自动断开校园网。

### 4.3 业务入口表

| 入口 | 主要能力 | 对外边界 |
| --- | --- | --- |
| `auth().identity()` | 统一身份登录、二次认证、恢复、注销 | 主账号与派生业务证明 |
| `auth().self_service()` | USEREG 登录、验证码、恢复、注销 | 第二个独立账号，保留所需访问通道证明 |
| `learn()` | 课程、公告、资料、讨论、作业 | 课程引用和资源引用带上下文验证 |
| `registrar()` | 课表、成绩、考试 | 学段与学期由证明结果确认 |
| `calendar()` | 学校校历图片、Learn 当前/后续学期时间线 | 不因内部当前经由 Learn 就暴露 Learn 协议 |
| `news()` | 新闻目录、搜索、详情、收藏及订阅读取 | 新闻选择值由上游目录或列表产生 |
| `service_hall()` | 待我处理、退回、已办、草稿、抄送、分阶段流程、服务目录 | 与网络学堂作业分开建模 |
| `library()` | 场馆/楼层/分区、开放时段、座位、插座 | 不把场馆根 ID 当座位分区 ID |
| `classrooms()` | 楼栋与教室状态 | 使用楼栋引用和周查询，不用裸列表下标 |
| `campus_card()` | 账户与有界交易读取 | 日期范围、交易完整性和账号绑定明确 |
| `electricity()` | 余额与缴费记录 | 保留独立业务证明与适用的缓存策略 |
| `self_service()` | USEREG 账号信息、用量、余额、设备管理 | 数据与第二个账号绑定；认证由 `auth().self_service()` 负责 |
| `network()` | 本机 TUNet/srun 状态、显式连接与断开 | 本地网络操作；不维护第三个账号会话 |
| `network().profiles()` | 可选保存、选择、编辑、删除连接资料与自动填写准备 | 保存不是在线证明，自动填写不自动提交 |

## 5. 类型、错误和结果约定

### 5.1 将业务选项从字符串/布尔值改成类型

| 当前输入 | 拟议输入 | 需要保留的语义 |
| --- | --- | --- |
| `graduate: bool/Option<bool>` | `AcademicStageSelection::{Auto, Undergraduate, Graduate}` | 自动推测只是探测偏好，必须有服务证明；手动选择不可被静默覆盖 |
| `semester: String`，特殊值 `auto` | `SemesterSelection::{Current, Term(TermId)}` | 上游学期 ID 经过验证后才能用于请求 |
| `kind: String` | `TaskView` enum | 只开放现有只读视图，不把未知字符串拼入 URL |
| `service: String` | `Service` enum | 高层业务服务与内部认证目标可以不同 |
| `force_refresh: bool` | `ReadPolicy` | 强制刷新仍遵守请求门禁、缓存边界和恢复规则 |
| 五个座位时段参数 | `SeatWindowRef` | 来自当前账号已验证分区、日期和时段的组合 |
| 楼栋/设备下标 | `BuildingRef`、`DeviceRef` | 在 Rust 内校验当前账号、来源、目录版本和有效期 |

不是每个字符串都要包装：标题、教师姓名等展示文本仍用 `String`。优先给会决定路径、账号、时间范围或可变操作目标的字段加类型。

`ArticleRef`、`TaskRef`、`CourseRef`、`PageCursor` 等若包含运行时选择能力，字段应私有，或是只能由 Rust 返回的 opaque 句柄。不能仅把 `String` 改成 `struct Id(pub String)` 就认为来源绑定已经完成。纯业务 ID 与有权限含义的运行时引用要分别建模。

日期使用明确的校园日期类型，时间戳表达时区/UTC；校园周与学期保留独立类型。金额在 SDK 内优先用明确单位的整数或受控十进制类型，并说明未知值，不让 FFI 的 `f64` 成为核心金额契约。

### 5.2 Rust 必须产生结构化错误

拟议统一入口为 `Result<T, Error>`。`Error` 至少具有服务、稳定类别、固定错误码和脱敏诊断 ID。内部保留足够的分类信息，但公开 `Display`、`Debug` 和 `source()` 不能携带认证 URL、ticket、Cookie、原始 body 或密码。

需要覆盖的类别：

- 无会话/会话明确过期、需要用户交互、账号/资源上下文不匹配。
- 网络失败、服务暂不可用、限流与 `retry_after`。
- 输入无效、响应未确认/结构改变、结果不完整、能力不支持。
- 存储不可用、操作取消、一次性操作结果不明确。

保留 `SessionExpired` 与普通网络/解析失败的区别。不能在迁移时统一捕获后全部映射为“需要重新登录”。

错误分类应在仍然持有底层错误证据的位置生成。不要先把错误格式化成中文，再通过匹配中文恢复枚举。Flutter 负责把结构化类别映射为展示文本，Rust 仍负责所有恢复判断。

`RetryAdvice` 可以提示等待或用户操作，但它不授权宿主自动重放认证请求。内部是否允许重试仍由操作类别、失败阶段和实际派发证据决定。未知结果不得被标记为安全重试。

### 5.3 统一读取的来源语义，保留业务差异

对可缓存的数据使用统一的 `ReadResult<T>`。元信息应能表达：数据的观测时间、来自实时读取还是缓存、缓存新鲜度、刷新失败原因。字段间的有效组合由 Rust 构造函数约束。

区分三件事：

1. **调用失败**：没有可用、账号匹配且经过验证的数据，返回 `Err`。
2. **返回旧缓存**：只有显式允许缓存退路时才能返回；包含缓存来源及导致回退的结构化失败，不能标成 live。
3. **完整读取的空集合**：上游明确确认没有记录，才是成功空结果。

`ReadPolicy` 的建议语义：

- `CacheOnly`：不访问网络；没有合法缓存就是明确错误。
- `PreferFreshCache`：使用服务允许范围内的新鲜缓存，否则尝试实时读取。
- `Refresh`：要求实时读取，失败时不伪装成缓存成功；验收使用此类语义。
- `RefreshOrCached`：显式允许在刷新失败时返回仍符合该业务缓存规则的旧数据，并携带失败原因。

策略不能改变服务本身是否允许缓存。例如实时座位和网络在线状态不因为调用者选择某策略就变成长期缓存。实时专用接口可以直接返回带观测时间的业务结果，不必给所有返回值机械套同一个壳。

Rust 的泛型结果在 FFI 中用具体 DTO 表达，例如 `NewsPageResultDto { data, metadata }`。公共 Rust 类型不用 `Dto` 后缀，避免把桥接限制带进 SDK。

### 5.4 分页与完整性是公共契约的一部分

分页查询返回一页时，清楚区分页面读取成功、上游报告总数、是否存在续页以及整体列表完成度。游标至少绑定服务、账号、筛选条件和必要的运行时版本；不可跨账号/查询拼接。

承诺“全部待处理”“完整有界交易”的接口必须在达到完成条件后才返回完整结果。触及页数/条数上限时返回 `IncompleteResult`，或通过显式支持部分结果的 API 返回 `Coverage::Partial`。不要复用 `Vec<T>` 隐去这个区别。

THOS 的待办首页计数不一定包含退回事项，`reported_pending_count` 必须与实际合并后的条数分别保留。校园卡交易范围、分页去重和完整性证明同样不能在通用分页抽象中丢掉。

## 6. 认证和网络服务的专项设计

完整状态模型、存储键、操作影响矩阵和迁移顺序见 [双账号与本地网络连接设计](auth-account-network-design.md)。这里给出主方案中的边界约定。

### 6.1 两个账号域各自拥有交互流程

`AuthDomain` 只有 `Identity` 与 `SelfService` 两个账号域。每个域一期支持一个活动账号，均可为空；统一身份 A 与网络自助 B 可以不同。业务结果必须验证所属域的目标账号，不能把所有服务都强制绑定到同一个 username。

统一身份输入包括其凭据、学段选择、信任设备与保存凭据选项。网络自助使用自己的凭据和验证码流程。信任设备、保存密码与允许自动恢复分别建模。

```rust
// 设计示意；credentials 来自用户输入，不在源码中放入真实凭据。
let step = client.auth().identity().login(login_request).await?;
match step {
    AuthStep::Authenticated(session) => show_account(session),
    AuthStep::Challenge(challenge) => request_user_interaction(challenge),
}
```

网络自助对应 `client.auth().self_service()`。挑战至少绑定认证域、账号、服务目标、账号/访问通道代次、有效期和当前阶段；验证码图片是短期交互数据，不写入通用缓存。

提交交互需要关联当前的 `ChallengeHandle`。Rust 在每次操作前检查账号、会话代次、服务目标、有效期和当前状态。主认证与校园卡/电费等服务追加认证可以共用交互模型，但不能混用挑战。

- `send_code` 只由显式用户动作触发；一次提交的结果不明确时不自动重发。
- `submit` 与会话变更互斥串行；挑战消费后不能再使用旧句柄。
- `cancel` 只取消对应的未完成交互，不把其他已证明服务一并注销。
- 需要目标服务密码时返回受作用域约束的交互请求，不把一次性导航 URL 或密码边界暴露给 UI。
- 凭据类型禁用敏感内容的 `Debug`/序列化；临时密码生命周期由 Rust 管理。敏感值清理不能只存在于 CLI feature 中。

### 6.2 分别表达账号状态与访问条件

建议分别提供：

- `AuthStatus`：两个 `AccountAuthStatus`，分别表达账号资料、会话证明和恢复状态；不包含 TUNet 账号槽。
- `ServiceStatus`：业务当前是否可访问，以及会话或路由的阻塞原因；不以一个全局 `authenticated` 表示所有业务。

USEREG 当前实现仍通过统一身份/WebVPN 取得访问通道；第二个业务账号不意味着它已经支持无前置条件的直连。它的业务主体可以是 B，访问通道可以依赖 A。换掉 A 时，其旧在线上下文应失效/重证，但不能因此遗忘 B 的独立账号资料。

状态必须来自运行时证明，不能由外部设置 `authenticated = true`。对应某服务不可用不应覆盖其他服务的有效状态；本地账号元信息与缓存也不等于身份认证成功。

`capabilities()` 公开业务能力、访问性质和当前可用性。图标、首页顺序、中文文案 key 等由宿主展示层维护。低层 `RuntimeServiceProofs` 一类布尔集合不作为外部构造接口。

### 6.3 校园网是本地连接能力

校园网没有需要 SDK 长期维护/续期的第三个业务账号会话。公开 API 为 `network().status/connect/disconnect`；内部临时 challenge、连接后验证和短期目标引用仍然保留，但不持久化为 Auth token 或 `Authenticated` 账号。连接资料按 `NetworkAccessMethod::{Portal, SystemWifiEap}` 区分：前者的口令只进入门户协议，后者的口令只可交给具备 OS 能力与用户授权的平台适配器。应用资料保存不等于系统网络配置已经写入或验证。

`network().profiles()` 可保存资料名称、账号输入、连接方式以及用户选择保存的密码，并提供应用内自动填写。保存、填写和提交三个动作独立：自动填写不自动发送连接请求，不自动借用或同步统一身份/USEREG 的密码。Portal 连接时可以提交资料引用，由 Rust 在当前操作内读取门户密码。Tsinghua Secure 的系统配置只有在对应平台适配器有能力并取得 OS 授权时才能自动提交；其他平台应明确引导用户在系统 Wi-Fi 设置中完成，不能把“已保存/已填写”显示成“已连接”。

Tsinghua Secure Wi-Fi 接入由操作系统管理。已通过 Wi-Fi 层在线时可以直接显示经验证的本机网络状态，不需要额外制造门户登录；EAP 密码不拿来自动尝试 srun。

网络状态结果需标明观测范围：当前请求出口、本机物理网卡对应地址，或其他明确指定的目标。出口在线不能替代本机在线证明；无法判断应保持 `Unknown`。

`network().disconnect` 针对经证明的本地连接；`self_service().disconnect_device` 针对第二个账号名下明确选定的设备。两者均显式执行，不混入账号恢复，也不能仅凭“看到在线”就授予断开能力。

## 7. 内部状态和并发应如何拆

### 7.1 保留一个共同所有者，拆分服务状态

先从当前运行时抽出私有 engine，逐步让其持有：

- `AccountSlots`：统一身份与网络自助各自的账号、会话代次和注销权威。
- `SessionCoordinator`：门户和业务服务证明，记录业务账号与访问通道的不同依赖。
- 各服务状态：`LearnState`、`RegistrarState`、`NewsState`、`ServiceHallState`、`LibraryState`、`SelfServiceState` 等。
- `LocalNetworkState`：连接资料选择、短期连接操作与网络观测，独立于 Auth 会话注册表。
- 共享 transport、恢复规则、私有存储与脱敏诊断。

服务状态只拥有自身的查询上下文、适配器和缓存句柄。调用过程由 engine 完成前置证明，再传入该服务实际需要的窄上下文。逐步消除“任何业务子模块都可以 `use super::*` 并修改整个 Runtime”的依赖方式。

例如 THOS 应接收一个已验证的账号/INFO 访问上下文与自己的 `ServiceHallState`，不需要能修改成绩缓存、校园卡密码边界和 UI 偏好。返回业务模型后，由旧桥接兼容层映射为 `ThosPendingDto`。

错误分类、恢复预算和来源验证仍在 Rust engine/业务模块内。不能为了模块独立，把这些决策推回 Flutter。

### 7.2 共享 transport 与门禁的性质不能改变

现有 `CampusHttpTransport::clone` 共享 Cookie jar 和派发计数器。这种内部共享是需要保留的；禁止的是复制成彼此独立的认证运行时，而不是禁止所有 `Arc` 或 transport clone。

请求门禁目前是进程内共享对象。重构后仍保持：

- 最多 4 个读取派发等待响应头，认证提交和一次性导航使用独占入口。
- 每个重定向跳转也经过同一 transport/gate。
- 429/503 与 `Retry-After` 的退避在共享上下文内生效，其他成功响应不能擦除退避。
- 不恢复无依据的固定 2/3 秒延迟；诊断节奏只由显式配置启用。
- 不能在按服务拆对象时给每个对象独立的 4 槽门禁，也不能每个重试创建新客户端。

初期继续保持进程级门禁的范围。若将来支持多个显式客户端实例，它们也不能通过实例数量放大总派发上限；门禁作用域改变需要单独审查。

### 7.3 取消、切换账号与过期返回

每次业务操作捕获其所属认证域、账号及相关会话/访问通道代次。两个认证域分别管理失效；换统一身份时，依赖该访问通道的 USEREG 在线上下文也需失效或重新验证，但不因此遗忘其独立账号资料。旧请求不能回写新作用域。

本地校园网操作捕获连接资料版本和本机网络接口/地址观测代次，使用独立的有效性检查；不把一次连接成功写为 `AccountSession::Authenticated`。

取消发生在普通 GET 与一次性认证派发后，处理方式不同。后者可能已经被服务器消费，必须保留“结果未确认/不得重放”的证据。一个依赖 handoff 已失败时，其他页面读取不能各自启动同一认证链。

跨进程的恢复 lease、注销权威和私有目录锁同样属于这条链路，不随“拆出 Client”而简化为一个内存布尔值。

## 8. 存储、宿主配置与 App 专属功能

### 8.1 配置由宿主声明，执行与校验由 Rust 负责

建议 `ClientBuilder` 仅开放有意义的宿主选项：应用身份、私有存储根、网络超时、显式诊断节奏、有限的日志/事件订阅。校方来源白名单、协议路径和一次性导航策略仍由 SDK 管理，不开放任意 URL 覆盖。

SDK 仅发出 `tracing` 事件，不主动注册宿主全局 subscriber。CLI 自己安装终端/报告日志；Flutter 包在显式初始化中安装应用选择的日志出口。构造 SDK 不默认读 `THYOU_*` 环境变量。

存储至少区分：按认证域/账号隔离的业务缓存、受控会话快照、信任设备元数据、两个认证域各自的可选凭据、本机 Portal/EAP 连接资料及其各自可选密码。即使四种用途使用相同 username，也使用不同存储键和用途字段。OS 原生 Wi-Fi 凭据不由 SDK 资料记录代替；更改应用资料不能隐式覆写 OS 配置。不能把上述用途合并为一个可随意导出的 `ClientState` JSON。默认使用内存，宿主选择持久化后仍须遵守各项 opt-in。

### 8.2 THYou 迁移需要专门兼容路径

当前 `THYou` 字符串不全是品牌文案。部分是目录名、环境变量兼容项、加密附加认证数据/格式域的一部分。全局替换会让已保存的数据无法验证，或者破坏账号隔离。

THYou 的旧目录、加密格式、设备指纹和注销 lease 由显式的旧存储迁移器处理：只在匹配的宿主命名空间内读取，校验账号与格式后决定迁移；失败明确报告，不静默重新登录、不复制原始凭据到新日志。

通用 SDK 不默认扫描 THYou 或其他 App 容器。旧环境变量只在 THYou 兼容入口/CLI 参数层映射到新配置，不能成为所有 SDK 消费者的隐式配置源。

### 8.3 页面功能放回应用层，业务聚合保留在 Rust

- 图标 key、本地化 key、默认可见性、首页紧凑模式和区块排序属于应用展示配置。
- `overview` 的跨服务读取与部分失败聚合若 App 仍需要，保留在薄的应用 Rust 适配层中，调用 SDK 的业务方法；不要重新复制网络/认证实现。
- 页面偏好如果沿用 Rust 持久化，也放在应用 Rust 适配层；它不必成为通用 SDK 的公开业务类型。
- 过渡期 FFI 可保留独立标识的 `legacy`/应用兼容模块，帮助现有 App 迁移。最终通用 Flutter 包不能以 THYou 首页模型作为主要入口。

这不是要把校园业务逻辑搬到 Flutter。Flutter 仍只负责显示、用户输入和高层调用；数据来源、账号绑定、缓存有效性与会话恢复继续由 Rust 决定。

## 9. 最终仓库与包组织

以下是目标目录，不要求第一步立刻移动：

```text
TsinghuaKit/
├── rust/
│   ├── Cargo.toml                    # 当前 workspace root
│   ├── Cargo.lock
│   ├── crates/
│   │   └── tsinghua-kit/             # 当前 SDK 类型包；目标继续扩展此 crate
│   └── src/                          # 旧 FFI Runtime；后续迁至独立 bridge package
├── packages/
│   └── tsinghua_kit/                 # Flutter package
│       ├── pubspec.yaml
│       ├── lib/
│       │   ├── tsinghua_kit.dart       # 唯一推荐的 Dart 导入入口
│       │   └── src/rust/              # 自动生成的 Dart 绑定
│       ├── rust/                     # package: tsinghua_kit_ffi
│       │   └── src/
│       │       ├── api/              # 只含桥接允许的接口、DTO 和转换
│       │       └── frb_generated.rs  # 自动生成
│       ├── android/ ios/ macos/ linux/ windows/
│       └── cargokit/
├── tools/
│   └── tsinghua-kit-check/           # 单独的 CLI package
└── docs/
```

依赖方向固定为：

```text
Flutter UI → Flutter 包 → Rust FFI → Rust SDK → 私有协议与基础设施
CLI --------------------------------↑
其他 Rust 程序 ---------------------↑
```

Rust SDK 不依赖 FFI、Flutter、终端提示和报告格式。FFI 也不靠 SDK 的原始 Cookie/transport 接口实现功能；它必须只用 SDK 的公共业务契约。

### 9.1 Flutter 包自己维护桥接一致性

Rust FFI 源码、生成的 Rust glue、生成的 Dart 代码、FRB 版本和 native library 名称一起发布。使用者通常只需：

```dart
import 'package:tsinghua_kit/tsinghua_kit.dart';
```

这将替代让每个 App 直接拥有 `lib/src/rust/` 并运行 codegen 的默认接入方式。Dart 的手写 facade 只做 SDK 初始化、参数适配和高层调用，不执行 HTTP、解析、认证恢复或业务缓存。

Rust 中的泛型、错误、借用型服务句柄不用直接透传 FRB。FFI 采用单一 opaque handle 和明确 DTO，内部持有同一个 SDK Client。公开的 Dart facade 可以按服务组织方法，生成层保留适合 FRB 的扁平方法。

第一轮必须验证 FRB 2.13 对带负载枚举、自定义错误和 opaque handle 的实际生成结果。若自定义 `Result` 不能保留结构化错误，就使用明确的结果 DTO/枚举携带错误；不退回仅有错误文本的稳定 API。

### 9.2 移动目录是一次完整发布变更

迁移到目标布局时，需要一起更新：

- workspace members、Cargo.lock、package/library 名称、native 输出文件。
- Android/CMake、iOS/macOS podspec 和 Cargokit 的 manifest 路径与库名。
- FRB 的 `rust_root`、`rust_input`、Dart 输出位置和动态库装载信息。
- Flutter Git 依赖的 `path: packages/tsinghua_kit`、锁文件，以及 App 的源码定位/验收脚本。
- GitHub Pages 的文档命令与产物目录，改成 workspace 下 `cargo doc -p tsinghua_kit --locked --no-deps` 和 `target/doc`。

具体 native library 是否更名在这一发布中明确决定，所有装载方保持一致。不能只把 `rust/` 移走后期待旧 Cargokit 脚本自动找到它，也不能从 SDK re-export FFI 来保留旧名字，因为那会造成依赖环。

### 9.3 CLI 成为实际公共 SDK 消费者

CLI 保留现有用例 ID、定向执行、脱敏报告、人工耗时排除和失败依赖传播。临时验证使用一个显式创建的 Client，凭据交互与会话变更串行，真实请求仍走共享 transport。

CLI 不再编译在 Flutter API 模块中，也不在通用客户端构造时自动启动。调试 App 内验收继续复用 App 已登录的同一 Client，由显式调试入口触发。

如果现有 WebVPN/业务证明单独用例需要额外观测，新增范围很小、只读、脱敏的 readiness/diagnostics 接口；不要为了迁出 CLI 而把整个运行时、Cookie 或“设置已认证标志”的函数公开。

## 10. 实施顺序与每阶段完成标准

这是一轮 SDK 边界改造，按约 8–12 个可独立审阅的变更组织更合适。下面是依赖顺序，不是工期承诺。物理分包在共享实现完成解耦后执行。

| 阶段 | 工作内容 | 必须保持 | 完成标准 |
| --- | --- | --- | --- |
| P0：基线与设计 | 固定当前修订、公开符号清单、76 个方法迁移表；标记 fixture/旧 API | 当前 App 与 tag 继续可用 | 本文与清单可审查，每个现有操作有明确去向 |
| P1：接口契约验证 | 定义两个 Auth 域、Portal/EAP 独立连接资料/观测、错误、读取元信息、挑战与资源引用；验证 FRB 和平台能力表达 | 不改实际认证请求顺序 | A/B/Portal/EAP 可用同名不同口令且互不串用；无 TUNet Auth 状态；不支持的系统连接能力有明确结果；最小桥接契约可生成 |
| P2：抽出共享 engine | 从 `api` 抽出不依赖 DTO 的状态/服务实现；明确宿主配置、日志与存储边界 | 旧 `CampusRuntime` 签名与业务保护继续有效 | 旧入口只委托同一 engine；新入口不依赖桥接模块；构造阶段没有隐式在线动作 |
| P3a：首个完整服务切片 | 优先迁移 THOS 读取及其完整分页、退回计数、错误和缓存来源 | 已修复的 THU Info 待办语义 | 新 SDK 方法、旧 DTO 包装和针对该切片的契约验证都贯通 |
| P3b：其余读取分批迁移 | INFO；Learn/教务；图书馆/教室；校园卡/电费，按独立批次推进 | 各服务账号/来源/路径证明与缓存策略 | 每批在方法清单标记已迁移，没有普通方法与 `_result` 的语义分裂 |
| P4：交互与网络边界（进行中） | 统一身份/自助交互、域级注销恢复；完成 Client 内 Portal/EAP 资料、显式密码填充和宿主目录普通 JSON 持久化，Portal connector、按平台能力适配系统 Wi-Fi；连接观测/操作从 Auth 移出；迁移本机资料旧 key | 串行认证、禁止不明确重放、用途/账号隔离、尊重 OS 授权 | 保存/填写不发连接请求；EAP 不走 srun；网络连接不生成账号会话；平台能力、账号变化和依赖失效均可验证 |
| P5a：物理分包 | SDK/FFI/CLI 形成 workspace；内部协议测试归位 | 核心依赖不反向指向 FFI/CLI | 纯 Rust SDK 依赖图不含 FRB，桥接和 CLI 只经业务/受限诊断入口调用 |
| P5b：Flutter 消费迁移 | 包内生成 Rust/Dart 绑定，迁移 App imports/gateway、Cargokit、锁文件、源码定位器 | 一个已登录 Client 贯穿 App 生命周期 | App 不再自带另一份 SDK 桥接源码；目标平台构建与运行验证有记录 |
| P6：文档与发布 | 新 Rustdoc、迁移指南、版本对齐、发布候选版本、最终删减旧导出 | 历史 release 可被继续锁定使用 | 变更清单、迁移验证、文档与实际版本一致，才发布新稳定版本 |

### 10.1 兼容层只朝一个方向依赖

过渡期允许：

```text
旧 CampusRuntime / 旧 DTO → 共享 engine / 新业务模型
新 Client / 新服务 API    → 同一共享 engine / 新业务模型
```

过渡期不应建立：

```text
新核心 SDK → 旧 Flutter Runtime → 又回调新核心 SDK
```

先在单 crate 内逐域完成这种依赖整理，再拆 package。旧入口与新入口可以同时存在，但在一次 App 会话内只使用同一个 engine 实例；不能为了对照结果同时创建两套已登录运行时进行真实请求。

所谓兼容包装是单向类型转换和调用委托，不是复制两份 HTTP/解析/恢复代码。新的类型化错误也不能由旧 `String` 错误猜测产生，必须让共享业务实现首先返回结构化错误，再由旧包装生成旧显示文本。

### 10.2 版本边界要诚实

`v0.1.x` 保留为现有 App 可锁定的版本。正式收窄 root exports、改变 crate 布局和默认构造副作用时，以 `v0.2.0` 作为明确的破坏性更新，先经过候选版本。

不能因为底层类型“原本打算是内部的”，就在 patch release 中把已经公开的类型改成 private。公开仓库可能已有未知消费者，本地只有 App 和测试的使用记录不能证明没有外部使用者。

core crate、Flutter package、FFI 与发布 tag 建议在初期对齐发布版本，并明确锁定 FRB 生成器版本。某个兼容模块若暂留，应标记迁移去向和移除版本，不无限保留始终失败的入口。

最终不让 core 通过 re-export FFI 保持旧 `api::runtime` 路径；纯 Rust 消费者按迁移表更新。Flutter 的兼容接口在 FFI/包的兼容层维护。

### 10.3 下一批代码改造的具体范围

建议第一批实现只落地 P1 与 THOS 切片所需的公共类型：

1. 先确定 `AuthDomain::{Identity, SelfService}`、双账号状态、`NetworkProfile`/`NetworkObservation` 及失效矩阵，再确定 `Error`、`ReadPolicy`、`ReadResult`、`Service`、`TaskView` 和 THOS 业务模型。
2. 将 THOS 业务返回值与 FFI DTO 分开，使旧方法仍可使用同一份结果映射。
3. 提供 `Client::service_hall().pending(...)` 的最小完整路径，并验证仍共享现有账号、transport 与分页证明。
4. 做最小 FRB 生成实验，确定错误/枚举在 Dart 中的稳定表达。

这一批不同时移动插件目录、改 native 库名或重写整个登录链。THOS 是合适的首个切片，因为它已经有相对独立的状态模块，也直接对应之前修复的用户问题，可以检验新 API 是否确实更容易正确使用。

## 11. 验证与发布门槛

### 11.1 现有协议测试保留，新公共契约单独建立

内部解析器/请求计划测试迁入所属模块的 `#[cfg(test)]` 测试树，继续使用已有脱敏 fixture。Rust 的普通集成测试不能访问 `pub(crate)`，所以不能只收窄可见性后仍把这些测试原样留在 `tests/`。

`tests/` 保留真正从外部使用 SDK 的契约测试：构造、认证交互、公共读取结果、错误与缓存策略。不要为测试开一个生产可启用的“强制已认证/注入任意 Cookie”公共 feature。

新增迁移验证重点包括：

| 场景 | 需要证实的性质 |
| --- | --- |
| `Client::build` | 不自动登录、不发校方 HTTP、不默认读取其他 App 存储、不抢全局日志 |
| 同一 Client 调用多个服务 | transport 共享，两个账号权威分别绑定，认证交互仍串行 |
| 统一身份 A、网络自助 B | 两者可不同，业务响应验证 B；访问通道仍验证 A 的上下文 |
| 两个账号与连接资料使用同 username | 密码、注销/恢复标记、缓存和加密用途仍独立 |
| 资料保存/自动填写/重启 | 不产生登录/连接请求，不由保存资料推断网络在线 |
| 身份成功、某业务证明失败 | 身份/其他服务不被错误地标记为全部失效 |
| 切换账号后旧请求返回 | 旧结果不能回写新会话、缓存或资源引用 |
| 一次性请求超时/取消 | 保留未知结果，不自动重放 |
| 429/503 | 共享退避生效，独立读取不能越过门禁 |
| 实时失败但有缓存 | 只有明确允许退路时返回，来源和失败原因真实 |
| 分页中断/达到上限 | 不伪装完整空列表或完整交易 |
| 过期/跨账号资源引用 | Rust 拒绝，不靠 Flutter 校验维持边界 |
| USEREG 图片验证码 | 图片短期展示，提交关联正确挑战，不复用旧登录页面 |
| TUNet 与 Secure Wi-Fi | 门户状态与本机/出口范围明确，不自动用统一密码登录 |
| Tsinghua Secure EAP 资料 | 不调用 srun；系统适配器按平台/授权报告能力，应用资料变更不声称覆盖 OS Wi-Fi 配置 |
| 单域注销/遗忘连接资料/断开网络 | 各自作用明确；断网不被当作两个账号的凭据过期 |
| Flutter codegen | 没有额外内部类型/方法，Rust/Dart/native 库版本一致 |

按项目规则使用已有台账，只执行当前新增或失败的验证项；相同版本和范围已经通过的验证不重复跑。新增迁移路径应有自己的验证记录。新版本的真实账号证明必须明确标注本次修订和实际执行范围，不能从旧报告复制“通过”。

真实账号验证继续遵守只读、单进程、同一 Rust Client/Runtime、认证交互串行、有界调度。App 内验证优先复用已登录实例；不可用时记录阻塞，不新建凭据仓库或绕过 transport 探测接口。

### 11.2 工程检查应直接对应改动

- 纯 SDK 的依赖图中不出现 `flutter_rust_bridge`、Dart/终端提示依赖。
- 外部 Rust 示例只 import 公共入口，能编译；需要在线访问的文档示例仅编译验证，不在 CI 自动登录。
- API 基线/兼容性检查将所有破坏性改动列出；新版本的破坏性变化不能用隐藏符号的方式绕过说明。
- FRB 生成后的结果做差异检查，生成文件不手工修改；消费示例证明 App 无需再对 SDK 运行 codegen。
- 触及平台 glue 时执行对应平台构建；未验证的平台明确记录，不把单平台成功写成全平台已验证。
- 对声明的 Rust 最低版本单独检查锁定依赖；当前 `rust-version = 1.85` 不能仅凭 stable 编译成功就认定已验证。

不要在每个纯目录移动步骤都重新跑全套线上验收。目录调整、公共类型变化、认证状态变化和发布平台变化，各自选择能证明对应风险的检查。

## 12. Rustdoc 与 GitHub Pages 的下一版组织

文档首先服务于外部调用者，推荐顺序：

1. 安装与最小示例：纯 Rust、Flutter 各自的接入方式。
2. 客户端生命周期、两个账号域、宿主存储、各自的会话恢复。
3. 两类账号登录、图片验证码和二次认证交互；校园网资料与连接单独说明。
4. 按服务浏览业务 API。
5. 错误、缓存来源、分页与完整性。
6. 只读操作和显式可变操作的区别。
7. 从 `v0.1.x` 迁移的对照表。

公共函数必须说明输入来源、认证前置条件、是否可能触发交互、缓存政策、分页范围、副作用、错误与取消后的重试限制。`SeatWindowRef` 等引用需要注明适用账号、有效期和如何获得。

在收窄后的 SDK API 上启用公共文档缺失检查和 Rustdoc 链接检查；不要在当前全部低层 exports 上机械补几百条无信息量注释。内部实现说明保留在源码中，常规用户文档不默认列出协议与测试兼容类型。

Pages 的主要入口指向已发布版本；`main` 文档可保留单独入口并明确标为开发版。页面标注 package 版本、tag/commit 和 feature 范围，避免 main 的文档被当作锁定 tag 的契约。

Rustdoc 内部导航使用可校验的 intra-doc links。共享 README 作为 crate 首页时，需要分别核对 GitHub 和生成站点上的相对链接；`cargo doc -D warnings` 不会验证所有普通 HTTP/Markdown 链接是否真实存在。

## 13. 不建议同时做的改造与主要风险

| 方案/风险 | 建议 |
| --- | --- |
| 仅把 25k 行文件拆成许多文件 | 可以作为准备动作，但必须同时缩小状态访问范围和公开契约 |
| 每个业务系统一个 crate | 暂不采用；增加发布与共享认证协调成本。当前三个消费边界足够 |
| 为了兼容，让纯 SDK 继续依赖 FFI | 不采用；旧 Rust 路径通过破坏性版本迁移，旧 Flutter 接口由适配层处理 |
| 用新 `Client` 套旧 `CampusRuntime` 就宣布完成 | 只能算临时入口；核心仍需摆脱 DTO、App 路径和桥接依赖 |
| 把所有错误改为同一个自定义 enum，但从显示文本推断 | 不采用；从原始错误证据一路保留分类 |
| 所有接口统一成任意 JSON/通用 request 方法 | 不采用；会失去输入、来源、凭据与业务完整性的边界 |
| 同时引入 actor、插件注册系统和每服务独立 runtime | 暂不采用；先维持单一 Runtime、双账号作用域与有界读取语义 |
| 删除旧测试来实现 API 私有化 | 不采用；迁移测试位置，保留其协议经验和失败样本 |
| 全局把 THYou 字符串替换成 TsinghuaKit | 不采用；文案、宿主目录、环境配置、加密格式分开处理 |
| 通过隐藏 Rustdoc 项目来减少 API 数量 | 不足以收窄 Rust/FRB 实际导出，需要可见性、包边界和生成结果一起处理 |

完成重构的判断标准是：新的 Rust 或 Flutter 使用者能理解一个 Client 中两个账号域与本机连接资料的区别，分辨需要登录、需要交互、暂时失败、缓存结果与完整空结果；服务句柄不会额外创建重复的账号会话。维护者修改校方解析器时，也无需连带修改所有外部使用者的类型依赖。

## 14. 当前方法逐项迁移表

下表覆盖基线中 `CampusRuntime` 的全部 76 个公开方法。目标栏是设计归属/方法族，最终精确签名由 P1 定稿；它不是已实现 API 清单。原签名和行号保存在 JSON 基线中。

当前已落地的 P3b INFO 新闻子集包括筛选目录、列表、搜索、详情、收藏、订阅规则和订阅文章页。新 SDK 将目录读取表示为 `client.news().catalog()`，列表/搜索统一到 `client.news().articles(NewsQuery, ReadPolicy)`，详情使用 `client.news().article(ArticleRef, ReadPolicy)`，其余只读入口为 `favorites()`、`subscriptions()` 和 `subscription_articles(NewsSubscriptionRef, page)`。来源与栏目条件必须取自同一 Client 最近一次目录结果；栏目目录不可完整确认时，结果元数据标为 partial。上述是 Rust SDK facade 实现状态；Flutter 消费迁移仍未完成。

Learn 已迁入课程、公告、作业列表/详情、资料列表/分类、讨论列表和显式资料保存。`CourseFileRef` 从同 Client 最近一次资料列表产生，绑定课程目录代次与资料列表代次并在五分钟后失效；保存前由 Runtime 对固定服务路径重读并确认该文件仍存在。资料目标路径只能由调用者显式传入，目标已存在时保存失败，不提供任意 URL、预览或诊断探测入口。资料和讨论最多读取 200 条，结果元信息会保留是否完整；讨论时间公开为来源标签，不猜时区。此批次仅实现 Rust SDK/engine facade，旧 FFI 方法保留兼容性，Flutter 尚未切换。

| 当前方法 | 拟议去向 | 迁移要点 |
| --- | --- | --- |
| `status` | `auth().status()` | 返回两个账号状态，业务可用性另行表达 |
| `session_recovery_retry_seconds` | `ServiceStatus / RetryAdvice` | 并入类型化恢复提示 |
| `resume_restored_session` | `auth().identity().resume` | 本地恢复与在线证明分开 |
| `login` | `auth().identity().login(LoginRequest)` | 具名选项与独立凭据类型 |
| `send_second_factor_code` | `auth().identity().send_code(ChallengeHandle, Method)` | 统一入口，保留主认证/服务 scope；显式发送 |
| `service_second_factor_methods` | `AuthChallenge::methods` | 从当前受约束挑战中读取 |
| `send_service_second_factor_code` | `auth().identity().send_code(ChallengeHandle, Method)` | 统一入口，保留主认证/服务 scope；显式发送 |
| `complete_service_second_factor` | `auth().identity().submit(ChallengeHandle, ChallengeInput)` | 校验代次、目标、有效期和一次性消费 |
| `complete_second_factor` | `auth().identity().submit(ChallengeHandle, ChallengeInput)` | 校验代次、目标、有效期和一次性消费 |
| `peek_overview` | `应用 Rust 聚合层` | 分别采用 CacheOnly/正常读取/显式刷新策略 |
| `load_experience_preferences` | `应用偏好适配层` | 页面配置从通用 SDK 移出 |
| `save_experience_preferences` | `应用偏好适配层` | 页面配置从通用 SDK 移出 |
| `load_thos_pending` | `service_hall().pending` | 完整待处理含退回；保留上游计数 |
| `load_thos_services` | `service_hall().services` | 只读完整服务目录 |
| `load_thos_task_list` | `service_hall().tasks(TaskView, ...)` | 用 enum 代替 kind 字符串 |
| `load_thos_phase_steps` | `service_hall().phase_details(&WorkflowTaskRef, ...)` | 引用来自当前 Client 最新完整的分阶段列表，并校验 Client 与代次 |
| `load_overview` | `应用 Rust 聚合层` | 分别采用 CacheOnly/正常读取/显式刷新策略 |
| `refresh_overview` | `应用 Rust 聚合层` | 分别采用 CacheOnly/正常读取/显式刷新策略 |
| `load_semester_schedule` | `registrar().semester_schedule()` | 返回完整学期、学段和缓存来源元信息 |
| `load_grades` | `registrar().grades()` | 保留学段、成绩类别和缓存来源元信息 |
| `load_learn_courses` | `learn().courses` | 返回业务课程和读取元信息 |
| `load_learn_files` | `learn().files(CourseRef)` | 保留文件来源上下文与 200 条完整性标记 |
| `load_learn_file_categories` | `learn().file_categories(CourseRef)` | 返回业务标签，不暴露当前无用的分类 selector |
| `load_learn_discussions` | `learn().discussions(CourseRef)` | 只读列表，保留 200 条完整性标记和源时间标签 |
| `download_learn_file` | `learn().save_file(CourseFileRef, &Path)` | Rust 重读并验证文件、固定来源和目标不覆盖 |
| `probe_learn_file_download` | `CLI/宿主受限诊断` | 不公开为一般 SDK 下载接口 |
| `load_learn_term_calendar` | `calendar().learn_terms()` | 当前与后续学期、教学周一和来源时间 |
| `load_school_calendar` | `calendar().school_calendar(SchoolCalendarQuery)` | 类型化年份/学期/语言，内部前置依赖保留 |
| `load_learn_homework` | `learn().homework(CourseRef, ...)` | 独立于网上服务大厅任务 |
| `load_learn_homework_detail` | `learn().homework_detail(HomeworkRef, ...)` | 引用与附件归属由 Rust 验证 |
| `load_learn_announcement_list` | `learn().announcements(CourseRef, ...)` | 合并旧简化返回值与完整结果入口 |
| `load_learn_announcements` | `learn().announcements(CourseRef, ...)` | 合并旧简化返回值与完整结果入口 |
| `load_exams` | `registrar().exams()` | Runtime 按学段选完整已验证来源；不公开无证据的日期筛选 |
| `load_info_news_catalog` | `news().filters` | 保持目录部分失败的可见性 |
| `load_info_news_subscriptions` | `news().subscriptions` | 只读规则与不透明引用 |
| `load_info_news_subscription_page` | `news().subscription_articles(SubscriptionRef, ...)` | 分页与订阅范围绑定 |
| `load_info_news_favorites` | `news().favorites` | 只读收藏文章 |
| `load_info_news` | `news().articles(NewsQuery, ...)` | 列表/搜索条件具名、分页统一 |
| `search_info_news` | `news().articles(NewsQuery, ...)` | 列表/搜索条件具名、分页统一 |
| `load_info_news_detail` | `news().article(ArticleRef, ReadPolicy)` | 合并，统一来源与失败语义 |
| `load_info_news_detail_result` | `news().article(ArticleRef, ReadPolicy)` | 合并，统一来源与失败语义 |
| `load_library_area_tree` | `library().directory` | 统一目录及其缓存元信息 |
| `load_library_area_tree_result` | `library().directory` | 统一目录及其缓存元信息 |
| `load_library_floors` | `library().floors(LibraryRef)` | 场馆与楼层类型分开 |
| `load_library_sections_for_day` | `library().sections(FloorRef, CampusDate)` | 分区不得以目录根替代 |
| `load_library_day_segments` | `library().time_windows(SectionRef, CampusDate)` | 合并时间参数与来源结果的重复接口 |
| `load_library_day_segments_result` | `library().time_windows(SectionRef, CampusDate)` | 合并时间参数与来源结果的重复接口 |
| `load_library_day_segments_for_day_result` | `library().time_windows(SectionRef, CampusDate)` | 合并时间参数与来源结果的重复接口 |
| `load_library_seats` | `library().seats(SeatWindowRef)` | 由已验证时段引用代替五个散参数 |
| `load_library_socket_status` | `library().sockets(SectionRef, ...)` | 实时状态，遵守原服务缓存限制 |
| `load_classroom_buildings` | `classrooms().buildings` | 统一返回有效楼栋引用 |
| `load_classroom_buildings_result` | `classrooms().buildings` | 统一返回有效楼栋引用 |
| `load_classroom_state` | `classrooms().availability(BuildingRef, WeekQuery)` | 移除公共下标选择 |
| `load_electricity_remainder` | `electricity().remainder()` | 合并业务值与来源返回接口 |
| `load_electricity_remainder_result` | `electricity().remainder()` | 合并业务值与来源返回接口 |
| `load_electricity_payment_history` | `electricity().payment_history()` | 保留只读缴费记录与来源 |
| `load_electricity_payment_history_result` | `electricity().payment_history()` | 保留只读缴费记录与来源 |
| `login_tunet` | `network().connect(NetworkConnectRequest)` | 显式输入或已保存资料；返回连接结果，不创建 Auth 会话 |
| `load_tunet_status` | `network().status(StatusScope)` | 明确请求出口/本机范围与未知状态 |
| `disconnect_tunet` | `network().disconnect(ConnectionTargetRef)` | 显式可变操作，不自动重放 |
| `load_campus_card_account_result` | `campus_card().account` | 统一业务证明和缓存来源 |
| `load_campus_card_account` | `campus_card().account` | 统一业务证明和缓存来源 |
| `load_campus_card_transactions` | `campus_card().transactions(TransactionQuery, ...)` | 有界日期范围、去重、完整性 |
| `load_campus_card_transactions_result` | `campus_card().transactions(TransactionQuery, ...)` | 有界日期范围、去重、完整性 |
| `usereg_login_phase` | `auth().self_service().status` | 以枚举表达图片/交互阶段 |
| `cancel_usereg_login` | `auth().self_service().cancel(ChallengeHandle)` | 仅取消此项未完成登录 |
| `start_usereg_login` | `auth().self_service().login(SelfServiceLoginRequest)` | 保留共享门户前置证明与独立密码 |
| `refresh_usereg_captcha` | `auth().self_service().refresh_captcha(ChallengeHandle)` | 显式刷新、旧图片上下文失效 |
| `complete_usereg_login` | `auth().self_service().submit(ChallengeHandle, Input)` | 验证码与当前挑战绑定 |
| `load_usereg_account` | `self_service().account` | 保留账户绑定 |
| `load_usereg_devices` | `self_service().devices` | 返回有范围的 DeviceRef |
| `load_usereg_balance` | `self_service().usage` | 包括服务提供的用量与余额 |
| `disconnect_usereg_device` | `self_service().disconnect_device(DeviceRef)` | 移除下标，明确目标与可变性质 |
| `service_catalog` | `Client::capabilities` | 业务支持/访问/证明状态；移出页面元数据 |
| `establish_service_session` | `Client::prepare_service(Service)` | 可选的提前准备入口，正常读取仍可按需建立 |
| `logout` | `auth().logout_all` 的旧接口适配 | 保持旧注销清理语义；新增 API 支持两个域分别注销；不自动断开网络 |

`runtime.rs` 中另外 6 个公开自由函数的去向：

| 当前函数 | 拟议去向 | 迁移要点 |
| --- | --- | --- |
| `backend_validation_second_factor_plan` | CLI 交互策略模块 | 不再出现在一般 Flutter API |
| `backend_validation_wechat_action` | CLI 交互策略模块 | 显式发送与提交策略保留 |
| `create_runtime` | Client::builder / FFI 兼容构造 | 宿主配置明确；从 core 移出应用日志与调试验收触发 |
| `infer_academic_stage` | 学段选择的提示工具或适配层 | 推测不能替代在线业务证明 |
| `create_backend_validation_runtime` | CLI 的 ClientBuilder 配置 | 新鲜临时会话；信任设备元数据与凭据/会话分开 |
| `run_backend_validation_batch` | CLI / App 显式调试验收入口 | 共享一个 Client，保留新增/失败用例调度 |

其他需要在同一版本说明的入口：

- `api::load_overview`：在破坏性版本中移除始终失败的旧入口，不再作为新用户起点。
- `api::init_app`：宿主/Flutter 显式初始化负责日志，不由纯 SDK 注册全局 subscriber。
- `api::list_service_catalog`：静态业务能力目录可以保留为 SDK 的纯查询；动态证明通过 `Client::capabilities` 暴露，页面元数据移出。
- `InMemoryCampusService` / `InMemoryData` 等兼容 fixture：从稳定生产 API 移除，内部测试使用明确的 fixture 类型。
- 公开的协议配置、请求计划、解析器、transport、会话 token/registry：按服务迁回私有实现；不能单靠 `doc(hidden)` 宣布兼容性未变化。

## 15. 2026-09-25 当前分支验证

RC20 工作副本恢复后，FFI 库根通过 feature 条件加载仓库生成的 `frb_generated.rs`；生成文件本身没有手工编辑。定向检查结果如下：

- `cargo check --locked -p tsinghua_kit_ffi --lib` 通过。
- `cargo build --locked -p tsinghua_kit_ffi --lib` 通过，并在 `rust/target/debug` 生成 `libtsinghua_kit.dylib`、`libtsinghua_kit.a` 和 `libtsinghua_kit.rlib`；Cargo 包名与原生库名不同，Cargokit 继续依据 `[lib].name` 选 artifact。
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps -p tsinghua_kit` 通过，文档输出是 SDK 而非 legacy FFI crate。
- 新增 USEREG 只读 API 的定向回归通过：SDK 的 SelfService 三类读取在第二账号未登录时都返回 `SessionRequired`；engine 的固定诊断码映射保留认证拒绝、会话过期、限流、网络失败和结构错误的区别；账号与设备 `Debug` 不包含测试中的账号、联系方式、地址或硬件地址后缀。
- `cargo fmt --manifest-path rust/Cargo.toml --all -- --check` 和 `git diff --check` 通过。
- 新增的 Cargokit Dart 单测尚未通过验证。`dart test test/cargo_test.dart test/crate_hash_test.dart` 在加载测试时因 Flutter Dart SDK 缺少 `frontend_server.dart.snapshot` 退出，测试代码没有执行。
- `cargo package --locked --allow-dirty -p tsinghua_kit` 未通过：crates.io 找不到尚未发布的 `tsinghua_kit_engine`。这是当前发布依赖次序问题，不是编译或 Rustdoc 失败；没有发起任何发布操作。

恢复后继续收敛了 SDK 的 Rust 外部入口：SDK crate 新增自己的 `Client` / `ClientBuilder` 门面，不再直接 re-export engine 的 `Client`、`ServiceHallClient` 或 `SelfServiceClient` 类型；认证入口归到 `client.auth().identity()` 和 `client.auth().self_service()`，服务数据入口继续由 `client.service_hall()`、`client.self_service()` 提供。Identity 和 SelfService 输入均不在 `Debug` 中暴露账号名或密码，其中新 SelfService 登录请求在 drop 时清零密码缓冲区。新 SDK 外部集成用例 7 项和公共导入用例 1 项通过，Rustdoc 使用 `-D warnings` 生成成功。此变化只证明 Rust crate 的入口边界，未迁移 Dart 包，也不代表所有业务服务已接入。

本次新增检查还包括：`cargo check --locked -p tsinghua_kit_ffi --lib` 通过；`cargo tree -p tsinghua_kit --edges normal` 不含 `flutter_rust_bridge`；在临时 `file://` Git 快照中建立独立 Rust 消费者并引用该 SDK，编译其双账号和服务读取入口通过。这个 smoke test 验证了 Git 源的相对 `engine` 路径解析，不验证 GitHub 上的提交或 tag。`cargo fmt --all -- --check` 与 `git diff --check` 通过。本次没有改动业务 HTTP/解析实现，没有重跑已有后端回归，没有进行真实账号请求。Flutter 主应用目前仍锁定 `TsinghuaKit v0.1.1` 的旧桥接入口。

`cargo check/build` 会报告 legacy engine 中的 unused/dead-code 与不可达分支 warning。本轮没有扩跑历史已通过的 6 项 SDK 集成测试或 9 项 `backend_refactor_` 测试，只执行了上面新加的定向用例；没有运行线上账号请求。Cargokit Dart 测试仍受本机 SDK 缺失文件阻塞。

## 16. 2026-09-25 六项兼容回归定向复验

RC20 恢复后的宽筛选曾启动 94 个 `backend_repair_` 兼容引擎测试，其中 6 项失败。本轮只对这 6 个失败项按唯一测试名重新运行，没有重跑其余通过项或全量引擎套件。六项最终均以 `1 passed; 0 failed` 通过：

- `backend_repair_existing_learn_and_registrar_api_does_not_repeat_portal_auth`
- `backend_repair_proven_portal_releases_private_credential_guard_for_next_expiry`
- `backend_repair_portal_boundary_second_stage_failure_does_not_repeat_either_post`
- `backend_repair_portal_boundary_two_stages_still_require_csrf_and_account_proof`
- `backend_repair_restored_portal_starts_at_target_sso_not_new_webvpn_login`
- `backend_repair_webvpn_password_entry_uses_one_trusted_device_recovery_before_retry`

前两项是合成上下文不一致：第一项原先用会恢复宿主持久会话的生产构造器，再叠加手工 Identity fixture；改为明确禁用持久会话的构造器，并为断言保留错误上下文。第二项保存的合成凭证标记为 Graduate，但测试 Runtime 明确使用 Undergraduate；将 fixture 学段对齐后，测试验证了门户证明会释放该次 Portal 凭证边界。

后两项导航测试原本把 Identity fixture 的 `Location` 指向真实 `info.tsinghua.edu.cn`，但测试随后仍期望本地 OAuth fixture 接收请求。初次诊断因此跟随了未携带 Cookie、ticket 或密码的公开 GET，并收到真实 Identity 登录页；这不是账号登录或业务验收。已把两个 `Location` 改为配置中的 loopback OAuth fixture，修复后的测试只访问本机 fixture。认证入口没有改动，来源与路径白名单没有放宽。中间两个 Portal 边界测试无源码改动，定向通过。

以上是 `tsinghua_kit_engine` 的合成单测结果，不代表校园账号线上验证。此次没有执行 1,600 余项全量引擎测试、Flutter 测试或真实账号请求。

六项复验之后，`RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps -p tsinghua_kit`、`cargo check --locked -p tsinghua_kit_ffi --lib`、`cargo fmt --manifest-path rust/Cargo.toml --all -- --check` 与 `git diff --check` 均通过。编译会继续显示拆分前 legacy engine 的 unused/dead-code 警告；没有在本轮清理这些历史告警。工作副本没有提交或推送。

## 17. 2026-09-25 SelfService 设备引用边界

新 SDK 的在线设备查询现在返回不透明 `DeviceRef`，显式断开必须传递列表行提供的引用。引用同时绑定 Engine Client UUID、完整列表代次和内部行号；这些内部值不通过 getter、`Debug`、序列化或 Flutter DTO 暴露。跨 Client 与过期引用在进入设备断开操作前返回 `ContextMismatch`。旧 FFI 的下标方法仍作为兼容入口，不由新 Rust facade 暴露。

每次成功刷新都会推进列表代次；刷新失败会清空设备目标并推进代次；Identity/USEREG 会话清理与全局注销也清空目标；已确认断开推进代次，断开结果不明确时会失效 USEREG 会话并丢弃所有引用。目标设备 ID、MAC 与 CSRF 仍只由 Runtime 和内部 adapter 持有。以上只约束 SelfService 账号名下的远端设备，不与 `network().disconnect` 的本机连接目标互换。

本次新增/受影响的定向验证均通过：engine 的引用 `Debug`、跨 Client 拒绝、旧代次派发前拒绝、失败刷新失效、认证过期失效、账号注销失效、完整刷新后成功断开失效 7 项；SDK 外部 `public_api` 集成测试确认 `DeviceRef` 可导入且 `SelfServiceClient::disconnect_device` 签名可用 1 项。严格 Rustdoc、FFI library `cargo check`、格式检查与 `git diff --check` 通过。Rust 编译仍输出拆分前 engine 的历史 unused/dead-code warnings；未运行全量 1,600 余项 engine 测试、Flutter/Dart 测试、真实账号验证或线上设备操作。工作副本仍未提交、未推送，剩余 P3b/P4/P5/P6 工作仍在进行。

## 18. 2026-09-25 INFO 新闻 facade 切片

首个新闻列表/搜索/详情切片新增 `news::NewsQuery`、`NewsArticle`、`NewsPage`、`ArticleRef`、`ArticleDetail` 和 `NewsClient`。列表与搜索共用 `ReadPolicy`；`CacheOnly` 不跨在线认证边界，`Refresh` 跳过缓存且不回退，`PreferFreshCache` 使用新缓存并允许合格的旧缓存回退，`RefreshOrCached` 尝试实时读取并允许缓存回退。结果把 live、Client 临时缓存、宿主持久缓存、缓存新鲜度、观测时间和已分类的刷新失败写入 `ReadResult` 元信息。旧 FFI DTO 方法仍走原签名和默认缓存行为。

后续补上只读新闻目录 facade。`news().catalog()` 使用既有严格解析和 INFO 账号/会话边界，不持久化目录缓存；新闻来源与栏目选项生成不透明引用，只能被创建它们的 Client 使用，并在下一次成功目录读取后失效。新闻列表来源 ID、列表栏目 ID、搜索栏目标签不再接受任意字符串，而从目录引用提供。来源目录经严格解析读取；栏目目录接口不可用时，结果只包含可从近期新闻响应中确认的候选项（读取候选失败或没有候选时可以为空），并通过 `NewsCatalogCoverage::Partial` 与 `ReadMetadata::coverage()` 表示目录不完整；不会把退化结果标成完整。SDK 外部 API 与 engine 单测覆盖筛选引用签名、跨 Client/旧目录引用网络前拒绝以及引用 Debug 脱敏。该改动不涉及真实账号访问。

详情选择器绑定 Client UUID、INFO 新闻链接代次和当前 Identity 缓存账号。账号不一致会使旧映射立即失效；INFO 会话注销、全局 Runtime reset、同一文章 ID 被不同链接映射、映射冲突和容量重置都会推进代次。详情引用在访问缓存或派发 HTTP 前验证，SDK 入口不接受裸文章 ID 或新闻链接。错误映射只使用 Rust 内部固定诊断码，外部 `Error` 不携带运行时错误字符串；新闻条目/引用的 Debug 输出不打印文章 ID、链接、正文或搜索词。

engine 定向验证覆盖查询边界、同账号链接替换、账号切换、冲突映射清理、跨 Client 文章/筛选引用、旧目录引用拒绝和订阅引用网络前验证。收藏接口保持完整 bounded pagination；订阅文章页面仅表示单页读取，不推断整体列表完整。它们只操作临时 Client 与合成 Runtime 上下文，没有账号登录和真实 INFO 请求。新闻收藏/订阅 facade 尚未迁移到 Flutter；其他 P3b 以外服务和整体重构继续进行中。

## 19. 2026-09-25 Learn 课程内容 facade 扩展

本轮在同一 Learn 门面下新增 `files(CourseRef)`、`file_categories(CourseRef)`、`discussions(CourseRef)` 和 `save_file(CourseFileRef, &Path)`。外部 SDK crate 提供自己的 `LearnClient` 适配器，并重新导出稳定的业务类型；使用者不需要引用内部 engine Client 或 `CampusRuntime`。课程资料引用隐藏 course/file selector，绑定创建 Client、当前课程目录、最近资料列表和五分钟时限。Identity 登录/二次认证完成/注销以及课程目录刷新会使旧上下文失效；运行时依然验证真实课程、Identity 用户、Learn handoff 和共享 Cookie jar。下载会再次读取课程资料列表，固定到既有 Learn 下载实现，经 `CampusHttpTransport` 请求，并使用已验证的显式目的地保护检查，不覆盖既有文件。

资料分类 API 只返回标签，因为当前公共 facade 没有按类别读取资料的操作；原 selector 留在 engine DTO 映射边界。资料和讨论在源端达到 200 条时返回部分数据及 `ReadCoverage::Partial(ReadLimitReached)`；不完整结果不能误当完整集合。讨论 selector 因当前没有帖子详情/发帖接口而不向消费者暴露。文件名建议经既有净化逻辑生成，Debug 对标题、描述、时间标签、讨论内容及 selector 脱敏。服务旧入口仍以 `String` 报错，所以外部 facade 只可靠地区分当前 Identity 状态与一般服务不可用；没有根据本地化错误文本猜测网络或限流原因。

新增定向验证：Learn file ref 的 Client/代次/时限校验与 Debug 脱敏，文件和讨论 DTO 映射的完整/部分覆盖，资料/分类/讨论文本与 selector 脱敏，以及跨 Client 的资料保存引用在登录和文件系统访问前拒绝；公共 SDK integration 测试验证这些类型和方法可从 `tsinghua_kit::learn` 外部入口使用。上述用例通过。SDK/FFI 定向检查、严格 Rustdoc、定向 `cargo test`、格式检查和 `git diff --check` 通过；engine 构建仍带有拆分前的 unused/dead-code warnings。未运行全量测试、后端真实账号回归或线上 Learn 请求，没有改动生成的 FRB 文件，也没有提交或推送。

后续仍需先将 Learn Runtime 的错误从 `String` 改为协议来源的结构化分类，并向 Learn 读取 API 增加显式缓存策略；再迁移学期/课程日历和其余服务 facade。该 facade 扩展没有完成 Flutter 消费迁移、帖子详情/发帖、文件预览或外部 crate 发布。

## 20. 2026-09-25 Registrar 与校历 facade

Rust SDK 新增 `registrar()`，提供 `semester_schedule()`、`grades()`、`exams()`。结果均使用 `ReadResult<T>`：保留 Runtime 给出的 live/cache 来源、观测时间和 fresh/stale 分类。 stale 结果若 Runtime 明确带刷新错误，则只映射为稳定的 `ServiceUnavailable` 提示；不解析中文旧错误。课表映射把校历日期规范化为 `NaiveDate`、日程时间解析为 UTC `DateTime`，检查学段、日期、事件类型、成绩数值与考试报告行数。公共 `Exam` 不暴露年份猜测；来源未提供完整日期时，原始排期文字保留在 `schedule_label`。

学期时间线与学校校历图片归入 `calendar()`。`learn_terms()` 读取当前/后续学期并校验来源标记、重复学期 ID、起止日期、周一开课对齐与周数。`school_calendar(SchoolCalendarQuery)` 使用枚举表达 Autumn/Spring 和中文/英文；`latest()` 由服务器选择最近发布年份，`for_year()` 在构造时验证年份。返回图片仍由 Rust 的共享 transport 获取，SDK 映射边界再核对选择值、来源、JPEG 头尾、尺寸与 12 MiB 上限；Debug 只输出字节长度，不输出图片内容或校方 URL。

新增定向 engine 验证：Registrar 元数据来源/缓存新鲜度、学段/成绩映射、完整学期日程/考试映射和文本脱敏 4 项；Learn 学期周对齐、重复 ID 拒绝和 Debug 脱敏 2 项；校历图片选择绑定与 JPEG 脱敏 1 项。外部 SDK `public_api` 测试 3 项验证 calendar/registrar facade 可导入、异步签名可编译以及年份选择校验。以上均通过。`RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps -p tsinghua_kit`、`cargo check --locked -p tsinghua_kit_ffi --lib`、`cargo fmt --all -- --check` 和 `git diff --check` 也通过。错误来源目前仍是旧 Runtime `String`，因此没有把教务/校历线上网络、限流或解析错误强行细分；没有账号登录或线上请求，没有改 FRB 生成文件，也没有提交或推送。

## 21. 2026-09-25 服务大厅只读 facade 扩展

`service_hall()` 现提供统一的 `ServiceHallReadPolicy::{CacheOnly, PreferFreshCache, Refresh}`，并保留 `PendingReadPolicy` 作为兼容别名。除既有 `pending()` 外，新增 `services()`、`tasks(TaskView, policy)` 和 `phase_details(&WorkflowTaskRef, policy)`。任务类别由 Rust enum 固定到已办、草稿、抄送、阶段事项四条只读路径；目录和任务列表使用 `ReadResult` 保留 live/cache 来源、观测时间与分页完整性。THOS Runtime 的 cache-only 分支只查同一 Identity、INFO 会话及共享 Cookie jar 绑定的 60 秒完整内存快照，没有命中时明确返回 `CacheMiss`。

`WorkflowTaskRef` 隐藏稳定 selector，并绑定生成它的 Client 与阶段列表代次。只有完整阶段列表返回可用于详情读取的引用；跨 Client、刷新阶段列表后、注销或重新开始 Identity 认证后，在发出阶段详情请求前返回 `ContextMismatch`。Runtime 仍二次验证当前账号的完整阶段列表和 selector。服务 ID、阶段聚合 ID、工作项 ID 与上游 URL 不进入公开结果；任务、服务目录和阶段数据的 `Debug` 输出只保留结构性摘要。

新增外部 `public_api` 覆盖服务目录、各任务视图和阶段详情方法签名，并验证未登录 cache-only 查询返回 `SessionRequired`。engine 合成单测验证 WorkflowTaskRef 的 Client/代次/view 绑定和脱敏，以及跨 Client/旧代次阶段引用在请求前被拒绝。此切片未登录真实账号、未访问线上 THOS、未运行全量 engine 测试，且旧 Runtime 字符串错误仍只映射为当前 Identity 状态或通用 `ServiceUnavailable`；请求、认证 handoff、分页和 HTML/JSON 解析继续由既有 Runtime、`ThosClient` 和共享 transport 负责。后续仍需将协议级错误分类下沉至其产生位置，并继续校园卡、电费及本机网络资料 API。

## 22. 2026-09-25 Library 只读 facade

Rust SDK 新增 `library()`，覆盖场馆目录、楼层、按校区日期选择的座位分区、开放时段、座位可用性和独立插座状态。`LibraryRef`、`FloorRef`、`SectionRef`、`SeatWindowRef` 与 `SeatRef` 隐藏服务 ID，并绑定 Client、目录代次及父级选择；重复读取目录、重新选择楼层/分区、切换 Identity 或注销会令旧的后代引用在 HTTP 请求之前失效。Runtime 继续二次核对根场馆、楼层、分区和所选日期/时间段。今天与明天在 Rust 内按校区 UTC+8 解析成明确日期；新 Runtime 入口保持该日期经过认证 handoff 不变，午夜之后已过期的请求会明确失败。

目录和开放时段使用既有账号绑定缓存：新鲜缓存优先，实时刷新失败时才允许符合保留期限的 stale 回退；`ReadResult` 显示 live/cache 来源、观测时间、新鲜度和刷新失败。当前 Library Runtime 没有逐调用的 `ReadPolicy` 参数，因此 facade 如实保留 Runtime 管理策略，没有承诺 CacheOnly 或强制 Refresh。楼层、分区、座位与插座为实时读取；所有成功集合都经过 SDK 映射边界的唯一 ID、标签、数量和时间区间校验。座位 ID 保留在不可调试输出的 `SeatRef` 中。插座 endpoint 仍是单独请求，其返回 ID 只在 Rust 内按当前 `LibraryAvailability` 合并；不匹配或重复 ID 拒绝整个响应，缺少某个插座记录明确映射为 `Unknown`。

新增外部 `public_api` 覆盖完整异步调用链，并验证无 Identity 会话时目录读取返回 `SessionRequired`。engine fixture 单测覆盖不透明引用的 Client/代次绑定和目录重复/异常值拒绝；Runtime 新增精确校区日期入口，沿用已存在的 Library cache、层级验证和共享 `CampusHttpTransport`。此切片没有真实账号登录、线上请求、全量 engine 测试、FRB 生成文件修改或 App 消费迁移；旧 Runtime `String` 错误仍限制网络/HTTP/限流/服务解析的细分，facade 只按认证状态映射稳定错误，其余明确返回 `ServiceUnavailable`。


## 23. 2026-09-25 Classroom 只读 facade

Rust SDK 新增 `classrooms()`，提供楼栋目录和按 `BuildingRef` 读取周教室状态。楼栋选择引用隐藏 Runtime 列表下标，并绑定创建它的 Client 与最近一次目录代次；重新读取目录会使旧引用在任何认证 handoff 或请求之前失效。调用者可以使用楼栋返回的默认周，或构造范围为 1–100 的 `ClassroomWeek`。周矩阵保持实时读取；楼栋目录沿用 Runtime 账号绑定缓存，结果通过 `ReadResult` 保留 live、Client cache 或 persistent cache 来源、新鲜度和 stale 刷新失败。

Facade 在 Rust 映射边界检查楼栋名与周号、周列表唯一性、请求周合法性、七个连续且周一开始的日期、唯一教室名和恰好 42 个时段状态。未知状态保留为 `ClassroomSlotStatus::Unknown`，不折算成空闲。公开矩阵不再重复暴露 Runtime DTO 中实际等于请求周的 `current_week_number`，只报告请求周及服务接受的周集合。缓存元信息解析已抽为 Library 与 Classroom 共用的严格 mapper；未知来源/状态、损坏时间戳和明显位于未来的时间戳仍返回 `InvalidResponse`。

新增 engine 定向验证 3 项覆盖楼栋引用脱敏、跨 Client/旧目录代次的引用在 Runtime 前拒绝、完整矩阵映射、时段数错误拒绝，以及跨 Library/Classroom 的 cache source、新鲜度、刷新失败映射。SDK `public_api` 新增异步调用签名编译覆盖、周值边界验证和未登录时返回 `Service::Classrooms / SessionRequired` 的测试。此切片没有真实账号登录、线上请求、全量 engine 测试、FRB 生成文件修改或 Flutter 消费迁移。旧 Classroom Runtime 入口仍返回 `String`，所以网络与业务失败除身份状态外明确收敛为 `ServiceUnavailable`；不从本地化错误猜测网络或解析类别。

## 24. 2026-09-25 CampusCard 只读 facade 与服务交互

Rust SDK 新增 `campus_card()`，提供 `account()` 与 `transactions(CampusCardTransactionRange)`。账户和流水继续使用 Runtime 现有账号绑定缓存，公共结果经 `ReadResult` 报告 live、Client cache 或 persistent cache 来源、新鲜度、原始观测时间以及 stale 刷新失败；校园卡余额不被折叠成缺少来源信息的普通 DTO。

交易查询由 SDK 接收严格日期、固定交易类型和最多 31 天范围，不接收账号序列号、内部交易 ID 或页码。Runtime 按已观察协议串行分页，只有证明请求范围完整才返回；SDK 再校验响应范围、交易日期、记录边界与内部 ID 唯一性，然后丢弃 ID。没有完整性证据时返回明确错误，不能把第一页或空失败变成完整结果。

初次校园卡 SSO 如果确实返回目标密码表单，SDK 的当前 Client 可以报告 `CampusCardInteraction::PasswordRequired`。密码只通过 `CampusCardPasswordRequest` 提交：Runtime 以短时限、Identity 用户、固定目标来源和当前 Client 内存挑战校验；验证前先消费挑战，认证请求结果未知时不允许再提交。临时明文及 SM2 wire 密文使用 Rust 零化缓冲区。若门户转而要求二次认证，`auth().identity().interaction()` 提供验证方式；用户仍需显式发送或提交验证码。TUNet 本机连接资料及其连接结果仍完全不进入 Auth。

CampusCard Runtime 当前入口仍产生 legacy `String` 错误，因此 SDK 只基于可验证的 Identity 会话/交互状态返回 `SessionRequired`、`SessionExpired`、`InteractionRequired` 或 `InteractionInProgress`；Authenticated 状态下的其他失败明确使用稳定 `ServiceUnavailable`，不以本地化字符串猜 HTTP、限流或解析类别。

此切片新增日期范围与密码 Debug/零化边界、无 Identity 时的 SDK 失败、卡片 challenge 一次性消费等定向回归。未执行校园账号登录或真实卡片服务请求，也没有运行全量约 1,600 项 engine suite。它只完成 CampusCard Rust SDK facade，没有迁移 Flutter/FFI 调用；校园网资料持久化/自动填写/OS EAP、能力目录、账号域独立恢复/注销、Runtime 错误类型整理、干净 GitHub 消费验证与真实 App 接入仍未完成。

## 25. 2026-09-25 Electricity 只读 facade

Rust SDK 新增 `electricity()`，公开 `remainder()` 和 `payment_history()` 两个只读调用。两个结果都通过 `ReadResult` 保留现有 Runtime 的 live/cache 来源、缓存新鲜度、原始观测时间及 stale 回退标记；账号绑定、新鲜/保留期限、Identity/WebVPN handoff 和请求仍由现有 Rust Runtime 与共享 transport 管理，SDK 没有另建缓存或网络路径。

电费余额和缴费金额继续使用服务返回的有限 `f64` 数值，文档明确不标注未知单位或比例。缴费记录只公开经严格检查的时间标签、金额原值和状态；未有语义依据的 legacy 原始列与序号不进入 SDK 类型或 `Debug`。时间格式、有限值、数值范围、空表标记及状态标签在公开映射边界再次校验；经验证的空表作为完整的空集合返回。

现有 Runtime 入口仍以旧 `String` 表示失败，因此 facade 只按 Identity 状态返回 `SessionRequired`、`SessionExpired`、`InteractionRequired` 或 `InteractionInProgress`，其余失败返回稳定的 `ServiceUnavailable`；不解析错误文案来猜测网络、限流或业务解析类别。此 facade 不改变 Flutter/FFI 调用，不声称已迁移 App，也未执行真实账号或线上电费读取。

定向验证已通过：engine 电费模型、Debug 脱敏、验证空表、缓存元信息映射及无 Identity 时两类读取拒绝共 6 项；SDK 外部 `public_api` 电费入口与未登录错误测试 1 项。`cargo check`（SDK/engine）、严格 Rustdoc、FFI library check、workspace fmt check 与 `git diff --check` 通过。Engine 编译仍显示拆分前 legacy 实现中的 unused/dead-code 警告及一个既存的不可达诊断分支；本轮没有运行全量 engine/Flutter 测试或线上账号验证。

## 26. 2026-09-25 Client 生命周期内网络资料 facade

新增 `network().profiles()`，提供本机资料 `list/save/update/prepare_fill/is_current/delete`。`NetworkProfileInput` 对资料标签、账号输入和方式做边界校验；密码只有在调用者显式调用 `save_password` 时才保存在 owning Client 的 Rust 内存中，并以 `Zeroizing<String>` 持有。资料摘要只报告是否有密码，不包含秘密。Debug 对输入、存储资料和 prepared handle 脱敏。资料用途明确区分 `Portal` 与 `SystemWifiEap`；两者均不创建、不恢复 Auth 账号，也不与 Identity 或 SelfService 资料联动。更改用户名/用途时，旧密码会清除，除非调用者为新值再次显式提供密码；修改或删除资料会使旧 prepared handle 失效。跨 Client 的 handle 不可复用。

当前 `prepare_fill` 只产生不含秘密的标签/用户名/连接方式和版本引用。若调用者明确需要填入密码，可用该引用调用 `password_for_fill`；Rust 会核对 Client 与资料版本，并返回 `NetworkProfilePassword`。此类型不可克隆、无序列化接口、Debug 脱敏并在 Rust 内零化；只有显式调用 `expose_for_form` 才会取得可放入 UI 字段的明文。资料仍只在 owning Client 生命周期内存在，进程重启后不会恢复，因此尚未形成持久资料闭环。尚未加入宿主选择的安全存储 adapter/opt-in、Flutter/FRB 生命周期集成、默认资料选择、Portal 连接执行、SystemWifiEap 系统配置和授权处理，也没有真实网络连接或账号请求。

engine 三项资料/密码定向测试及 SDK 外部 API 一项测试通过，覆盖用途/密码存储隔离、账号或用途变化清除旧密码、版本与 Client 绑定、Debug 脱敏，以及不创建 Auth 状态。TUNet 查询 facade 现在返回 `PortalObservation` 与 `PortalAddressRegistration`，准确表达门户对 Rust 查询时本机 IPv4 的登记结论；它不表示普通互联网连通性，也不表示 Tsinghua Secure 是否在线。未实现的通用 `NetworkObservation`/连接结果类型暂不从 SDK 公共模块导出，避免调用者把设计占位符当成可执行功能。后续应先实现显式 opt-in 且密钥归属清楚的跨进程安全存储和 Flutter/FRB 填充接入，再分别实现 Portal connector 与受平台能力/授权约束的 SystemWifiEap adapter。工作副本未提交或推送。

## 27. 2026-09-25 显式资料密码填充与 Portal 观察语义

网络资料新增 `password_for_fill(&PreparedNetworkInput)`。Rust 只在 prepared 引用属于当前 Client 且资料版本仍匹配时提供已显式保存的密码；未保存密码返回 `None`，跨 Client、修改或删除后的引用返回 `ContextMismatch`。结果是 `NetworkProfilePassword`，不可克隆、无 serde 接口、`Debug` 只显示是否存在，并以 `Zeroizing<String>` 持有；只有 UI 明确要求填入时调用 `expose_for_form()` 才会把明文复制到表单字段。常规 Profile DTO、列表和 `Debug` 不含密码。SDK 尚未集成 Flutter/FRB 表单流程，亦未提供跨进程安全存储。

重命名并收紧只读门户状态结果：`network().observe_portal_status()` 返回 `PortalObservation`，状态为 `Registered`、`NotRegistered` 或 `Unknown`，只表达 TUNet 门户对查询时本机 IPv4 的登记证据。它不再映射成设备联网 `Online/Offline`，因为 Tsinghua Secure 系统 EAP 可以在未登记 srun 时仍提供网络连接。通用连通性探测和连接执行尚未实现；相应占位类型不再由 `tsinghua_kit::network` 导出。

定向验证通过：engine 的资料密码脱敏/版本与用途隔离 3 项，Portal 登记状态与矛盾响应 2 项；SDK 外部资料密码填充/Debug 1 项和 curated public module 导入 1 项。SDK/engine/FFI 检查、严格 Rustdoc、格式及补丁检查通过。没有发起真实账号登录、Portal 登录/断开或校园网状态线上查询；Flutter/FRB 消费迁移仍未完成。历史 engine legacy unused/dead-code 与电费 unreachable-pattern warnings 仍存在。未提交或推送。

## 28. 2026-09-25 显式 opt-in 的网络资料持久化

本节更新第 26、27 节当时“资料只在 Client 生命周期内保存、尚无持久化”的状态。SDK 的默认策略仍是 `NetworkProfileStoragePolicy::MemoryOnly`；宿主只有显式将 `NetworkProfileStoragePolicy::encrypted_directory(root, namespace)` 传给 `ClientBuilder::network_profile_storage(...)`，才会启用跨 Client/进程恢复。namespace 是稳定的宿主应用标识，不应包含账号名。创建 Client 时只打开本地资料存储，不登录、不发网络请求；并发 Client 共享同一 namespace 会因独占锁明确失败，避免多个写入者互相覆盖。

当前 Unix 文件后端用 ChaCha20-Poly1305 加密资料元数据和可选保存口令，使用随机密钥、namespace 绑定的认证数据、原子文件替换、私有目录/文件权限和有界解码；篡改、格式不符、符号链接目录、存储读写错误均返回存储错误，不把损坏数据降级为空资料。`delete()` 现在返回 `Result<bool, Error>`，持久化失败不会伪报删除成功。非 Unix 平台返回 `Unsupported`。此后端的密钥与密文位于同一个宿主私有目录，不能当作 OS Keychain，也不能防御同一 OS 用户、管理员或可访问该目录的进程；OS Keychain/宿主 secret-store adapter 仍是后续工作。

持久化只恢复本机 Portal/EAP 表单资料及用户显式选择保存的口令，不恢复 Identity 或 SelfService 会话、Cookie、ticket、CSRF、门户登记状态或网络在线证明。两个 Auth 域仍彼此独立，校园网连接资料仍不构成第三个账号。密码只有在 UI 明确要求填入时，才通过当前 Client 和资料版本绑定的 `PreparedNetworkInput` 调用 `password_for_fill`；得到的 `NetworkProfilePassword` 不可克隆/序列化、Debug 脱敏并在 Rust 中零化，调用者仍需显式 `expose_for_form()`。

定向验证通过：加密资料往返/篡改拒绝、namespace 与 Client 独占、符号链接根拒绝 3 项 engine 测试；SDK 对表单准备与 Debug 脱敏、跨 Client/进程恢复且 Auth 仍 signed out、默认内存资料随 Client 释放 3 项外部 API 测试；另有 engine 对资料版本失效且不进入 Auth 状态的测试。没有进行真实账号登录、TUNet 查询或网络连接，也没有改 Flutter/FRB 桥接生成文件；Flutter 尚未消费这组 API，所以当前改动不代表 App 已能设置或填写校园网密码。

## 29. 2026-09-25 Auth 与网络资料 Flutter facade 首批接入

插件当前工作树版本升为 `0.2.0-alpha.1`。FRB bridge 现在在 `tsinghua_kit_ffi` crate 内定义自己的 DTO 和 Client/资料 opaque handle，再委托 Flutter-independent Rust SDK；FRB 生成的 Rust/Dart 文件由 `flutter_rust_bridge_codegen` 生成，没有手工修改生成产物。Dart 入口不公开生成的 `SdkErrorDto`，而映射为只含 `service`、稳定 `code`、`retryAfterMs` 和诊断 ID 的 `TsinghuaKitException`。桥接初始化共享单个 Future，初始化失败后允许后续 Client 创建重试。

新的 Dart facade 结构是 `TsinghuaKit.createClient()` → `client.auth.identity` / `client.auth.selfService` / `client.network.profiles`。它允许 Identity 与 SelfService 使用不同账号；`client.auth.selfService.logout()` 只清理 USEREG 当前账号选择、会话、验证码挑战和设备目标，保留 Identity 证明与共享 transport；`client.auth.identity.logout()` 退出 Identity 并撤销派生服务证明，仍被选择的 SelfService 账号保留为 `Expired`，因为其共享 Identity/WebVPN 通道已经关闭；`client.auth.logoutAll()` 明确退出两个账号域。三种动作都保留网络资料且不改变本机网络连接。网络资料仅提供 Portal/EAP 用途、profile CRUD、无密码列表 DTO、当前版本表单准备以及明确调用的密码填入。它不创建第三个 Auth 状态，不执行网络连接，也不改写操作系统 Wi-Fi。service-hall 已桥接 pending、目录、四种只读任务视图与阶段详情；INFO、Learn、Registrar、Calendar 首批业务 API 也已进入 facade。Library、Classroom、CampusCard、Electricity 的首批只读 API 已桥接；`client.auth.selfService.phase()` 已提供本地固定登录阶段；App 仍待整体迁移。

未知二次认证方法继续显示为 `SecondFactorMethod.unknown`；若调用方尝试发送或提交该项，Dart 在发起 FRB 请求前抛 `ArgumentError`，不退化为 TOTP。新增三项 Dart 单测覆盖支持的方法映射、未知方法保留和拒绝误发。macOS ARM64 临时 Flutter 消费者由 CocoaPods/Cargokit/Xcode Debug 成功构建并启动；运行时创建无登录 Client 后读到两个 `signedOut` 状态，传入无效 ProfileId 后得到公开的 `TsinghuaKitException(network:invalid_input)`。没有输入真实账号，也未发起校方 HTTP 请求。

验证：SelfService 单域退出通过 engine 定向测试证明其保留 Identity 认证状态、清空 USEREG 设备引用且不发请求；Identity 单域退出通过另一项定向测试证明它撤销 Identity 与 USEREG 在线证明、保留 B 的账号选择并将 B 置为 `Expired`，且不发请求。SDK/FFI `cargo check` 通过；旧 Runtime 的 `a_new_runtime_starts_signed_out_and_logout_is_idempotent` 测试改为使用唯一临时持久化根，直接运行通过，不会读取默认用户会话目录。Dart 格式、改动文件的 `dart analyze`、三个 `flutter test test/second_factor_method_test.dart` 用例通过；`flutter test --no-pub` 在 Cargokit build_tool 下的两个测试文件总计 3 项通过。直接 `dart test` 仍被当前 Flutter Dart SDK 中缺失的 `frontend_server.dart.snapshot` 阻断，但用 Flutter 测试 runner 已执行并通过同一组 build_tool 测试。更新 Identity 退出桥接后的临时 macOS 消费者已重新由 CocoaPods/Cargokit/Xcode Debug 构建并运行，确认双账号初始状态、两个单域退出调用及 Rust 错误到公开异常的映射。示例工程最初因机器上 Xcode 仅支持 macOS 12+、而 Flutter 模板默认 10.15 未能构建；将临时验收工程 Podfile/Xcode deployment target 调为 12 后构建及运行成功，这不是仓库插件源码失败。

本次没有把 App 从远端 `v0.1.1` 迁走。迁移前还需把 App 会使用的服务方法桥接到同一个 Rust Client，设计和实现 OS Keychain/宿主安全存储接口，并验证其余目标平台构建。`SystemWifiEap` 现在仍只是本机资料用途标签，不代表自动写入系统设置；Portal 登录与操作系统 EAP 配置都不在当前能力内。工作副本仍未提交或推送。


## 30. 2026-09-25 网上服务大厅待办 Dart/FRB 切片

`ClientHandle` 现在通过 public Rust SDK 的同一个 `Client` 调用 `service_hall().pending(policy)`；新增的 `client.serviceHall.pending(policy: ...)` 返回 `ReadResult<ServiceHallPending>`。调用者必须选择 `cacheOnly`、`preferFreshCache` 或 `refresh`，结果保留 live/cache 来源、新鲜度、UTC 观测时间、覆盖完整性、刷新失败码，以及首页单独报告的待办计数。Task DTO 只暴露显示字段，不暴露协议 selector；Rust `Debug` 对待办标题、状态、步骤和时间脱敏。会话缺失由 Rust 返回 `service_hall:session_required`，不会转换为空列表。

FRB Dart/Rust 文件由仓库锁定的 `flutter_rust_bridge_codegen` 生成。本轮新增 Rust FFI 单测覆盖部分读取元数据、stale fallback 来源/失败码、selector 与内容 Debug 脱敏，以及未登录时在读取前返回认证错误。另修正既有 `registrar_exam_contract` 集成测试：测试现在经公共 crate 导入已公开模块，不再用路径直接编译依赖 crate 私有 `crate::telemetry` 的源码；该目标 17 项通过。

验证：FRB codegen 通过；两个待办 FFI library 定向测试通过；Dart facade 静态分析通过。临时 macOS 消费者的待办方法现已完成联调，并通过类型化未登录门禁验证。README 与账号/网络设计文档已更新这一切片。THYou App 仍锁定 `v0.1.1`；不能为只迁移待办而另起一个 Client，接下来需要在同一 handle 上补齐其余 App 必需服务，再迁移旧 gateway 并删除 App 自带生成桥接。

## 31. 2026-09-25 网上服务大厅完整只读查询 Flutter facade

同一个 ClientHandle 现已提供服务目录、已完成事项、草稿、抄送事项、多阶段流程和阶段详情。Dart 以 ServiceHallTaskView 枚举限定视图，以显式缓存策略请求读取；所有成功结果统一保留来源、新鲜度、UTC 观测时间、覆盖完整性和 stale fallback 错误码。服务目录的上游总数与实际读到的条目数分别保留；任务列表的上游总数与实际项目数也分别保留。

FRB 不传输 THOS selector。阶段任务 DTO 仅携带由当前 ClientHandle 生成的随机不透明引用 ID，Rust 在 Client 内将它映射回公共 SDK 的 WorkflowTaskRef。引用只能从阶段列表返回，不能跨 Client 使用；未知或跨 Client 引用返回 service_hall:context_mismatch。重新实时读取或 stale refresh 后，旧引用映射会失效；同一新鲜缓存快照重复读取时继续复用原引用。未知任务视图返回 invalid_input，不会默认为阶段视图。阶段、目录和任务显示字符串的 DTO Debug 均隐藏正文。

验证：flutter_rust_bridge_codegen generate 通过；新增三个 FFI 定向测试覆盖目录和任务读取的未登录门禁、未知/跨 Client 阶段引用错误、未知视图拒绝；cargo check -p tsinghua_kit_ffi --lib 通过；Dart facade 定向 dart analyze 通过。临时 macOS 消费者经 CocoaPods/Cargokit/Xcode Debug 构建并运行，实际调用 pending、目录和四类任务视图，收到预期的 service_hall:session_required；未知视图收到 service_hall:invalid_input。该 smoke 没有登录或发出校方请求。消费者工程只为适配本机 Xcode 27 临时设为 macOS 12 部署目标。Flutter 当前另提示插件未采用 macOS Swift Package Manager，未来 Flutter 版本会将其升级为构建错误，需纳入插件平台迁移工作。

THYou App 仍锁定 v0.1.1，也还没有切换自己的 gateway。此完整 THOS 只读 facade 只解决同一 Client 的公共服务入口和安全上下文边界，不意味着网上服务大厅写操作、Portal 连接或操作系统 Wi-Fi 配置已经公开。

## 32. 2026-09-25 SelfService 账号数据 Flutter facade

同一个 ClientHandle 新增 SelfService 账号摘要、当前设备列表、用量/余额和显式设备断开。Dart 入口位于 client.selfService；认证登录、图片验证码、退出仍位于 client.auth.selfService，两个名字对应不同职责，且都使用同一个 Rust Client。账号、设备和用量读取返回带来源与观测时间的 ReadResult，服务端格式的流量、余额和结算日原样保留，不在 Dart 重新解析单位。Flutter account DTO 不包含登录用户名，避免把 SelfService 登录标识作为业务显示字段回传。

设备条目附带当前 ClientHandle 内随机生成的不透明引用。FRB 不发送服务端设备 ID 或下标；读取新设备列表、任何 Auth 变更或开始断开前会废弃旧引用。Rust SDK 再验证 Client、SelfService 会话与设备目录代次；断开是一次性操作，不自动重放不确定结果。没有有效 SelfService 会话时，业务读取返回 self_service_auth:session_required，不使用统一身份代替。

验证：新增三个 FFI 定向测试覆盖 SelfService 登录门禁、未知/跨 Client 设备引用错误以及账号/设备/余额 DTO Debug 脱敏；cargo check 与 FRB codegen 通过；Dart facade dart analyze 通过。临时 macOS 消费者在未登录状态下调用 account、onlineDevices 和 usage，均收到预期的类型化 SelfService 认证错误；没有触发校方请求。TsinghuaKit App 仍未迁移，设备断开的已登录真实服务路径尚未进行线上验收。

## 33. 2026-09-25 Registrar 与 Calendar Flutter facade

`ClientHandle` 在同一个 Rust SDK `Client` 上新增 Registrar 学期课表、成绩与考试读取，以及 Learn 学期时间线和学校校历图片读取。Dart 通过 `client.registrar` 与 `client.calendar` 调用；每个成功结果都保留 source、freshness、观测时间和覆盖状态。课表事件时间转换成 UTC `DateTime`，服务提供的日期标签继续使用 ISO 日期字符串，考试保留来源中的月/日而不补造年份。校历参数使用公开 enum 和可选年份；未知 semester/language 与服务年份范围外输入在 Rust 请求前拒绝，响应中的未知枚举则在 Dart 保留为 `unknown`。

验证：FRB 2.13.0 codegen 通过；`cargo check --locked -p tsinghua_kit_ffi --lib`、Registrar 三个未登录门禁测试、Learn 学期未登录门禁测试、校历未知 selector/越界年份拒绝测试以及 Rust 格式检查通过；Dart facade 定向 `dart analyze` 通过。临时 macOS 消费者由 CocoaPods/Cargokit/Xcode Debug 构建并运行，在同一个未登录 Client 上验证上述门禁与输入拒绝，输出 `TSINGHUA_KIT_REGISTRAR_FACADE_OK` 和 `TSINGHUA_KIT_CALENDAR_FACADE_OK`；没有登录，也没有发出有效的校历或学业数据请求。消费者全项目 `flutter analyze` 仍会命中模板测试遗留的 `MyApp` 未定义，因此本轮对实际 smoke 入口单独运行 `flutter analyze lib/main.dart`，结果通过。

尚未对已登录 Registrar/Learn 或真实学校校历发线上请求；这些结果只证明桥接类型、客户端门禁与无效输入路径，不代表当前学校接口已完成真实账号验收。macOS 插件仍提示尚未支持 Swift Package Manager，该问题与本切片的 Cargokit Debug 构建成功并存，须在 Flutter 升级前单独处理。THYou App 仍使用 `v0.1.1`，未切换到新包；工作副本未提交或推送。

## 34. 2026-09-25 INFO 新闻 Flutter facade

新增 `client.news`，通过同一 Rust Client 暴露目录、分页列表、搜索、详情、收藏、订阅规则和订阅文章页。`ReadPolicy`、`ReadResult` 与来源/新鲜度/覆盖元数据移入共享 Dart `read.dart`，让 News 使用通用缓存策略；service-hall 继续保留它不支持的专用策略 enum。

Rust 将目录返回的 source/channel、新闻页返回的 article、以及订阅列表返回的 subscription 各自绑定成 Client 内随机不透明引用。Flutter 不传 INFO selector、article ID、subscription ID 或自由 URL。刷新目录成功后旧筛选引用失效；刷新订阅成功后旧 subscription 引用失效；读取新新闻页、收藏或订阅文章成功后旧 article 引用失效；任何 Auth 变化也会清空这些引用。跨 Client 或不存在的引用在 Rust 内明确返回 `news:context_mismatch`。页号、每页大小、搜索词和分页边界由 Rust SDK 验证；搜索词只在显式搜索操作时进入 Rust，未写入日志/错误字段。

文章详情仍以 HTML 与摘要供宿主显示，但附件只返回名称，不泄露下载 URL。Rust 和 FFI `Debug` 对详情 HTML、摘要、订阅关键词及全部随机 selector 做脱敏；Dart facade 的调用参数接收 opaque reference 对象，不公开 selector 字段。搜索词在显式搜索调用时交给 Rust，但不会进入桥接错误 DTO 或这些类型的 `Debug` 输出。收藏只在 Rust 证明完整有界分页后返回。News 结果与其他服务共用一致的 `ReadResult` 元数据模型。

验证：新增三项 Rust FFI 定向测试，覆盖无会话门禁、无效页/搜索输入早期拒绝、未知文章/订阅引用拒绝与 Debug 脱敏；这三项均通过。`flutter_rust_bridge_codegen generate`、FFI library 编译、Dart facade 定向 `dart analyze` 和临时 macOS 消费者 Debug 构建/运行通过；同一个未登录 Client 验证目录、分页、收藏、订阅门禁与无效搜索，并输出 `TSINGHUA_KIT_NEWS_FACADE_OK`。全程没有账号登录或有效 INFO 新闻读取。consumer 入口的 `flutter analyze lib/main.dart` 通过；临时工程模板中的 `test/widget_test.dart` 仍引用不存在的 `MyApp`，所以整个临时工程分析不能作为通过记录。

真实登录后的目录、列表、文章详情、订阅与收藏尚未线上验收；这次只证明生成绑定、Client 边界、参数拒绝和无会话路径。Flutter macOS 插件仍提示未来需要迁移 Swift Package Manager 支持。App 仍固定在 `v0.1.1`，未切换 package；本地公共库工作副本继续保持未提交。

## 35. 2026-09-25 Learn 业务 Flutter facade

`ClientHandle` 现将同一个 Rust SDK Client 的 Learn 课程、公告、作业与详情、课程资料/分类、讨论和显式文件保存接入 `client.learn`。公开 Dart 类型不暴露 Rust/FRB DTO；课程、作业、文件选择都使用构造器私有的不透明引用。Rust 在 Client 内把引用绑定到当前目录与认证状态，成功刷新上游目录/列表后会替换关联引用。所有读结果继续提供共享 `ReadResult` 来源、新鲜度、观测时间和覆盖元数据。

带 `_utc` 的公告/作业时间由 SDK 输出 RFC3339 UTC，再在 Dart 转换为 UTC `DateTime`；讨论和资料页直接提供的日期标签保留原始文本，不推断时区。作业详情只返回正文和附件元数据，不暴露附件 URL。讨论和资料保留 200 条上限对应的 partial coverage。`saveFile` 必须接收宿主显式选定的目标路径；Rust 校验下载引用及目标路径并拒绝覆盖既有文件。Learn 的课程/公告读取沿用 Runtime 的缓存语义，作业、详情、资料、分类和讨论保持各自当前的来源/缓存语义，不在 Dart 重建。

验证：FRB 2.13.0 codegen 已重新生成 Rust 与 Dart 绑定；`cargo check --locked -p tsinghua_kit_ffi --lib` 和 `flutter analyze lib` 通过。临时 macOS Debug 消费者构建并运行成功；在同一个未登录 Client 上调用 `client.learn.courses()` 得到 `learn:session_required`，并输出 `TSINGHUA_KIT_LEARN_FACADE_OK`。这不包含真实登录或有效学校读取。Rust 编译仍显示 engine 既有的 72 条 warning。Learn 已进入 facade，但真实账号在线行为尚未验收。App 仍固定 `v0.1.1`，会话恢复与其他服务迁移仍阻止 App 切换。

## 36. 2026-09-25 Library、Classroom、CampusCard 与 Electricity Flutter facade

同一 `ClientHandle` 现暴露 Library 场馆/楼层/分区/开放时段/座位/插座、Classroom 楼栋/周状态、CampusCard 账户/不超过 31 天的完整流水，以及 Electricity 余额/完整缴费记录。Dart facade 将金额保留为整型单位或服务提供的数值，不接受自由学校 ID；目录和座位选择由 Client 内不透明引用关联。校园卡目标密码提示、提交与取消仍是统一身份 handoff 的一次性交互，不增加 Auth 账号槽。

验证：用当前本地包构建的 macOS Debug 消费者在同一个未登录 Client 上调用 Library、Classroom、CampusCard 和 Electricity 的新 API，均收到对应 `session_required`；错误输入路径仍按稳定 `invalid_input` 检查。`flutter analyze lib/main.dart` 与 `flutter run -d macos --debug` 通过，输出四个新 facade marker，没有登录或发起有效服务读取。构建提示该插件尚未支持 macOS Swift Package Manager；此项需在 Flutter 将提示升级为错误前迁移。本轮没有真实账号或线上数据验证。

这些 package facade 已覆盖迁移矩阵中对应的 Rust/Flutter 层，但 THYou App 仍固定 `v0.1.1`，没有消费本地未发布 API。Identity cookie 快照现可显式选择加密目录并重新验证；OS Keychain/宿主 secret store、SelfService 持久状态恢复、Portal 执行、OS EAP adapter 和整 App gateway 迁移仍是切换阻断项。

## 37. 2026-09-25 USEREG 登录阶段 facade

`client.auth.selfService.phase()` 现返回 `SelfServiceLoginPhase` 固定枚举，不再把旧 Runtime 字符串直接暴露到 Flutter。Rust 只检查当前进程里的验证码挑战；挑战过期、Identity 绑定变化或阶段未知都会要求重新开始显式登录。查询会清理失效的挑战，但不会发送请求、重提密码、自动刷新验证码或提交验证码。

验证：新增 FFI 定向测试覆盖 fresh Client 的 `RestartRequired`；Rust 格式检查、`flutter analyze lib`、FRB codegen 和临时 macOS Debug consumer 均通过。consumer 在未登录状态调用该方法得到 `restartRequired` 并输出 `TSINGHUA_KIT_SELFSERVICE_PHASE_OK`，同时其它已桥接服务的未登录门禁 smoke 通过。未登录入站没有发出服务请求；真实 USEREG 登录和验证码交互未测试。

## 38. 2026-09-25 Identity 二次认证交互查询 facade

`client.auth.identity.interaction()` 现在桥接 Rust `IdentityAuthClient::interaction()`。它返回当前显式登录或服务 handoff 暂停的 challenge、受支持认证方式和掩码手机号；没有 challenge 时返回 `null`。未知方法继续作为 `unknown` 展示，只有明确支持的方法可以提交；查询不发送验证码，也不启动登录。

验证：新增 FFI 定向测试证明 fresh Client 的交互查询返回空；FRB codegen、Rust 定向测试与 Dart 静态分析通过。临时 macOS Debug consumer 调用 `interaction()` 收到 `null` 并输出 `TSINGHUA_KIT_IDENTITY_INTERACTION_OK`，没有账号或网络请求。二次认证的真实服务端 challenge 和提交流程尚未线上验收。

## 39. 2026-09-25 本地学段提示 facade

`TsinghuaKit.suggestLoginStage(username)` 已跨 Rust/FRB/Dart 暴露为本地 Identity 登录提示。它只使用 Rust 已有的学号阶段规则，不要求 `Client`，不会建立认证状态或发出 HTTP 请求；无法识别的输入返回 `null`。FFI 将非穷尽 SDK enum 的三个已知变体逐一映射到 DTO，遇到未来未知变体同样返回 `None`，避免猜测学段。

验证：修正 DTO 映射后，定向 Rust 测试 `identity_login_stage_suggestion_is_local_and_nullable` 通过，覆盖研究生、本科生与未知输入。临时 macOS consumer 的有效与无效输入 marker 均通过，现有服务 facade smoke 也再次通过；本轮 `cargo fmt --all -- --check`、严格 SDK Rustdoc、`flutter analyze lib` 与 `git diff --check` 通过。该检查不登录，也不代表真实服务验证。`inferAcademicStage` 迁移状态已同步至矩阵；THYou App 尚未迁移。

## 40. 2026-09-26 Identity 显式会话恢复入口

Rust SDK 新增 `IdentitySessionStoragePolicy::{MemoryOnly, EncryptedDirectory}`，默认仍为 `MemoryOnly`。显式目录模式要求 cache 与 session 使用同一个 host-selected 私有目录；Rust 使用现有设备绑定、期限、加密快照、authority generation 与 logout tombstone 机制。快照中没有密码；新 Client 只恢复 Cookie jar 与 Identity account locator，并将 Identity 标记为 `RestoredUnverified`。首次校验必须由调用者显式调用 `identity.revalidate_restored_session()` / Flutter `client.auth.identity.revalidateRestoredSession()`；新建无会话 Client 调用时是本地 no-op。该实现不读取 legacy THYou 默认目录，bridge 将身份数据放在 host root 下独立的 `TsinghuaKit/identity-session/<namespace>` 路径。

此目录后端只用于迁移期验证，不是 OS Keychain：加密密钥仍与快照位于同一私有目录。SDK 在第一次成功的 Identity 持久化边界保存一次当时的共享 Cookie jar，并关闭后续快照写入；之后业务或 SelfService handoff 收到的 Cookie 仍在同一个进程内 jar 中使用，但不会刷新磁盘检查点。保存时已经存在于 jar 的 Cookie 仍可能被写入，因此这不是逐 Cookie 分区或完整账号隔离。新 Client 不据此恢复 SelfService 的账号状态或授权，SelfService 仍显示 `SignedOut` 并需独立显式登录。旧 envelope schema 1 快照会被拒绝，不做隐式迁移。细分 Cookie partition、SelfService 持久状态及 Keychain/host secret-store adapter 必须在发布稳定版前完成，不能把本策略当作最终双账号凭据存储设计。

验证：engine fixture 测试用合成 Cookie 快照重建 Client，确认 Identity 为 `RestoredUnverified`、SelfService 保持 `SignedOut`；FFI 测试确认显式持久目录构造、默认 Auth 状态和 fresh Client 重新验证均不触发网络；外部 SDK 集成测试确认 builder 与 async revalidation 是可消费 API。新增 network engine 测试拒绝空、`.`、`..`、路径穿越和账号样式 namespace，并确认 storage policy 的 Debug 不泄露路径或 namespace。FRB 2.13.0 由 codegen 重新生成；`cargo fmt --manifest-path rust/Cargo.toml --all -- --check`、`cargo check --locked --manifest-path rust/Cargo.toml -p tsinghua_kit_ffi --lib`、`RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --manifest-path rust/Cargo.toml -p tsinghua_kit`、SDK `flutter analyze lib` 和 `git diff --check` 通过。临时 macOS Debug consumer 在显式临时目录配置下创建 fresh Client，调用 revalidation 前后都得到 Identity 与 SelfService `SignedOut`，并输出 `TSINGHUA_KIT_IDENTITY_REVALIDATION_FRESH_SIGNED_OUT_OK`；没有登录或访问学校服务。consumer 的 Xcode deployment target 调整为本机支持的 macOS 12。没有真实账号、Keychain 或学校服务请求。

## 41. 2026-09-26 SDK Identity 快照写入边界

新 SDK Runtime 在一次成功的 Identity 持久化边界提交当时的共享 Cookie jar 后，禁用后续快照写入；业务和 SelfService handoff 仍在当前 Runtime 使用同一个内存 Cookie jar，但其后收到的 Cookie 不进入这份磁盘快照。旧兼容 Runtime 保留原来的响应级刷新语义。该检查点仍保存整个当时的 jar，不提供逐 Cookie 归属或严格账号分区；SelfService 账号状态不会因快照恢复。会话 envelope schema 升为 2，旧 schema 1 快照明确拒绝且不做自动迁移。

验证：定向 engine 测试 `backend_refactor_sdk_identity_snapshot_is_frozen_after_first_commit` 通过，确认首次提交后的 SelfService Cookie 不改变已保存快照，并确认新的 Identity 认证边界重置 one-shot 标记；`backend_refactor_session_envelope_v1_is_rejected_without_migration` 通过，确认旧 envelope 被拒绝。`cargo fmt --manifest-path rust/Cargo.toml --all -- --check`、FFI `cargo check`、严格 SDK Rustdoc、SDK `flutter analyze lib` 与 `git diff --check` 通过。编译仍报告 engine 现有 unused/dead-code 等警告；本轮没有重复执行已通过的定向测试，也没有进行真实账号登录。

## 42. 2026-09-26 固定 SDK 根导出名单

Rust SDK 的 `read`、`service_hall` 和 `self_service` 模块不再用通配符重新导出 engine 模块；每个模块在 `lib.rs` 明列当前支持的类型。新增 engine 内部类型不会因为一次实现层改动自动成为 TsinghuaKit 外部 API。实现仍由 engine 持有，但外部 crate 的可见项需经过 SDK 根入口显式选择；FRB 专用兼容 re-export 仍受独立的 `ffi-compat` feature 和隐藏模块边界约束。

验证：SDK `cargo check`、workspace `cargo fmt --check` 和 `git diff --check` 通过。此项仅收紧 Rust 导出名单，不改变现有 API 调用语义。

## 43. 2026-09-26 移除 FFI crate 中未编译的旧实现副本

`rust/src/lib.rs` 只编译 FRB 生成模块、SDK bridge adapter 和验收 CLI；协议实现已由 `tsinghua_kit_engine` 提供。清理了 FFI package 下未被 crate 声明引用、也未被验收程序 include 的 156 个旧实现和测试副本。Rust engine 和 SDK 源码成为当前唯一被 Cargo 编译的协议实现位置；旧基线分析仍链接到不可变 Git commit，避免文档证据因清理而断链。生成的 `frb_generated.rs`、`sdk_api.rs` 和 CLI 源码保留在 FFI package。

此次 `cargo check --locked --manifest-path rust/Cargo.toml -p tsinghua_kit_ffi --all-targets` 首次编译到两个旧示例，发现 `CampusRuntime::login` 已扩展参数而示例仍使用旧签名。示例现在明确使用自动学段、不信任新设备、不保存自动续接凭据后全目标检查通过。验证还包括 `cargo fmt --manifest-path rust/Cargo.toml --all -- --check` 与 `git diff --check`。engine 目前仍报告 72 条既有 warning；没有运行真实账号验证。

## 44. 2026-09-26 本机网络资料的平台安全存储密钥（后续方案，已废弃）

Auth 与本机网络资料继续分开：Identity 和 SelfService 仍是唯一账号域，Profile key 只用于加密 Portal/EAP 填写资料。Rust engine 新增 `KeychainEncryptedDirectory` policy；新 Flutter 工厂 `NetworkProfilePersistence.platformSecureStorage` 使用 Flutter Secure Storage 为每个 app namespace 保管随机 32-byte key，再将该 key短暂传入 `ClientHandle`。Rust 将接收的 key 包在 `Zeroizing` 中，隐藏 `Debug`，且加密 Profile envelope 标记 key 来源。Auth 会话持久化不复用 Profile key；Identity 旧目录快照仍保留原来的安全限制，不能据此实现 App 登录记忆。

显式从旧 `EncryptedDirectory` 切到平台安全存储模式时，engine 在 Profile store 独占锁内读取旧 key、解密数据、写入由宿主 key 加密的新 envelope，再移除同目录旧 key。若进程在新 envelope 已提交但旧 key 尚未移除时中断，envelope 的 key-source 标记使下一次启动使用宿主 key并清理遗留 key，不会误把资料当空数据。真实 keychain / keystore 项目值不进入 Rust 日志或 FFI 返回值。

定向验证：3 项 engine 测试覆盖新 key 不落盘、密文/错误 key 处理、旧存储迁移和“新数据写入后、旧 key 清除前中断”恢复；1 项 FFI 测试确认安全 key 可构造本地 Client 且两个 Auth slot 仍 `SignedOut`。FRB 2.13.0 codegen 已更新 Rust/Dart 绑定；Flutter API smoke 覆盖新 factory。`cargo fmt --check`、对应 Rust 测试、`flutter analyze lib`、`flutter test test/public_entrypoints_test.dart` 通过；engine 仍有约 72 条既有 warning。尚未在 macOS Keychain/Android Keystore 上运行插件集成测试，THYou 也还没有 production 网络 Profile UI 或生产 gateway 迁移。

## 45. 2026-09-26 Identity 快照宿主安全密钥（后续方案，已废弃）

Identity Cookie 快照新增 `IdentitySessionStoragePolicy::HostSecureStorage` 与 Flutter `IdentitySessionPersistence.platformSecureStorage`。Flutter Secure Storage 使用 `identity_session_key.<namespace>` 独立条目保管 32-byte key；NetworkProfile 继续使用另一个 `network_profile_key.<namespace>` 条目。Rust 接收时将 key 包在 `Zeroizing` 中，Client 生命周期内仅保留零化数组；`Debug` 固定显示 `[REDACTED]`。Rust 快照仍处于 app-private cache 目录，但旁边不会创建 snapshot key 文件。Key 长度或 policy 配置不合法会在 Client 构造时拒绝。

错误或已丢失的 host key 不会令 Rust 删除仍存在的加密 Identity 快照。正确 key 重启恢复后的 Identity 状态仍为 `RestoredUnverified`，SelfService 维持 `SignedOut`；只有显式 `identity.revalidateRestoredSession()` 才进行只读校验。该快照仍然包含首次成功检查点当时的共享 Cookie jar，不构成逐账号 Cookie 分区，也不持久化密码或 SelfService 会话。真实 Keychain/Keystore 集成和 key-loss UI 尚未验证。

验证：FRB 2.13.0 codegen 更新 Rust/Dart 绑定；Rust 定向测试覆盖 host-key 加密、无旁置 key 文件、key mismatch 保留快照和 Client 恢复状态；FFI 测试覆盖安全 key 长度及本地 Client 构造。`cargo check -p tsinghua_kit_ffi --lib`、`flutter analyze lib test/public_entrypoints_test.dart`、`flutter test test/public_entrypoints_test.dart`、Rust 格式和 `git diff --check` 通过。engine 仍输出既有 unused/dead-code warnings。本项没有真实 keychain、真实账号或学校网络验收。

## 46. 2026-09-26 Portal 显式连接 facade

本机网络 facade 现提供 `observePortalStatus()`、`connectPortal(profile:, password:)` 和 `disconnectPortal()`。显式连接必须使用 `client.network.profiles.prepareFill(id)` 返回的 `PreparedNetworkProfile`；Rust 再校验该引用属于同一 Client 且资料版本仍有效。调用者可以不给密码，让 Rust 使用用户先前显式保存的 Profile 密码；也可以传入一次性密码覆盖，此值仅交给本次 Rust 操作，不保存。`SystemWifiEap` 资料在连接前明确返回 `network:unsupported`，不会被发送到 TUNet。成功结果只表达 TUNet Portal 的已验证连接/断开操作，不改变 Identity 或 SelfService Auth 状态。

断开仅使用当前 Runtime 已验证并保存在内存中的 TUNet target。重启 Client、换本机 IPv4、单纯读取到在线状态都不会生成断开权；调用即消费该目标，模糊结果不自动重发。Rust 在 Portal 调用期间将密码包入零化容器，结构化错误仅返回稳定 service/code 与诊断 ID。

验证：FRB 2.13.0 codegen 已重新生成 Rust/Dart bindings；engine 定向测试覆盖无密码时要求交互、拒绝 EAP 资料、两个 Auth 域保持 signed out；FFI 定向测试覆盖同样的公开桥接错误和无已验证目标时拒绝断开。SDK/FFI `cargo check` 通过，Dart facade 与 public entrypoint 分析无误。尚未发出真实 TUNet 登录或断开请求，Tsinghua Secure 系统 EAP adapter 和 THYou App production gateway 迁移仍未实现。

## 47. 2026-09-26 service cache 与 Identity 快照分目录

Rust `ClientCachePolicy::Directory` 与 Flutter `ClientCachePersistence.directory` 现可独立选择业务读取缓存目录；默认仍是 Client 生命周期内的私有临时目录。Identity Cookie 快照继续由 `IdentitySessionStoragePolicy` 单独选择，NetworkProfile 又使用独立的 `NetworkProfileStoragePolicy`。Runtime 为缓存路径和账号恢复根分别执行范围校验，持久缓存仍限制在宿主提供的私有缓存根内；三种数据不会因复用同一 Client 而隐式合并目录。缓存只保存业务读取数据，不创建或证明登录状态。

验证：FFI 定向测试 `bridge_business_cache_and_identity_session_use_independent_directories` 使用互不相同的临时目录创建 Client，并确认两个 Auth 域保持 signed out；FRB 2.13.0 codegen 已更新构造参数；`cargo fmt`、Dart facade 分析及 public entrypoint 测试通过。THYou App 尚未创建使用该策略的 `TsinghuaKitClient`，也尚未迁移旧 Runtime。

## 48. 2026-09-26 Identity 登录的逐次凭据选择

Rust IdentityLoginRequest 与 Flutter IdentityAuthClient.login 新增默认关闭的 rememberCredentials。请求开启时，Rust 要求该 Client 已显式配置 Identity 持久化根，并只在认证成功后把 Identity 凭据写入已有的加密凭据库；无此配置时会在发送登录请求前返回 StorageUnavailable。密码不进入 Cookie snapshot、业务缓存、DTO 或日志；Session snapshot 自己仍不包含密码。当前 vault 使用应用私有目录中的本地加密密钥，并非操作系统 Keychain，也未与 SelfService 凭据隔离策略统一；这一安全级别必须在发布说明中明确，不能把 host-secure snapshot key 描述成 vault key。

THYou 旧登录表单也新增默认关闭的“在此设备保存统一身份凭据”复选项，不再将 rememberCredentials 固定为 true。网络自助仍没有记住凭据或跨进程恢复接口，App 生产认证路径仍通过旧 gateway，尚未使用新的 SDK 登录入口。

验证：engine 定向测试 backend_auth_identity_credential_opt_in_requires_storage_before_login 与 backend_auth_identity_login_request_debug_redacts_inputs_and_reports_choice，以及 FFI bridge_identity_credential_opt_in_requires_configured_storage，确认无 Identity 持久化配置时立即失败、保持账号 signed out 且错误/Debug 不含输入；FRB 2.13.0 codegen 重新生成登录签名，SDK Dart analyze 与 public-entrypoint 测试通过。THYou 的 login page、显式 opt-in / 默认关闭、启动失败后的 opt-in 路径及 Auth controller 定向测试通过。没有执行真实登录或保存真实凭据；SelfService 凭据、独立的 OS 安全凭据后端及 App SDK 迁移仍待实现。

## 49. 2026-09-26 Identity 与 SelfService 凭据库分离

Auth 密码存储现由独立 `CredentialStoragePolicy` / Flutter `AuthCredentialPersistence` 配置，不再与 Identity session snapshot 根目录耦合。engine 在一个应用命名空间下为 Identity 与 SelfService 创建独立凭据域；两边的同名账号不能读取或覆盖彼此的记录。两个登录入口的 `remember_credentials` 均默认关闭，只在成功认证后保存；关闭该选项会清除该账号之前保存的凭据。SelfService 可调用 `startSavedLogin(username:)` 在 Rust 内启动新的验证码流程，依然必须人工完成图片验证码；没有跨进程恢复 SelfService session 的行为。Identity 自动跨进程恢复仍需要单独启用 Identity session snapshot，且仅在恢复 Cookie 被明确判定为失效后按既有有界恢复门禁读取凭据。

Dart `IdentityAuthClient.login` 与 `SelfServiceAuthClient.startLogin` 已接受逐次保存选择；SelfService 增加 saved-login 与忘记凭据入口，成功提交结果报告非敏感的 `credentialsSaved`。凭据 vault 目前用 app-private 目录中的独立本地密钥加密，不等同操作系统 Keychain；session snapshot key、Profile key 与 credential key 不复用。THYou App 仍没有生产 SDK provider，不能把本节视作 App 已迁移。

验证：engine 的 `backend_auth_self_service_credential_opt_in_requires_separate_storage`、`backend_auth_credential_domains_do_not_share_same_named_account_records` 通过；FFI 的 SelfService 未配置凭据库拒绝 opt-in 与 credential/session 目录分离测试通过。严格 SDK Rustdoc、Flutter `analyze lib test/public_entrypoints_test.dart` 和 `test/public_entrypoints_test.dart` 通过。测试均为本地合成 fixture；没有真实校园登录或请求。THYou 的 72 个旧 gateway 入口和生产单 Runtime 迁移仍待完成。

## 50. 2026-09-26 统一采用宿主选择的普通 JSON 文件

用户明确选择不依赖系统凭据存储，也不需要本地加密。Flutter Secure Storage、HostSecureStorage 与平台 Keychain/Keystore API 均不属于当前方案。显式保存的 Identity 与 SelfService 凭据、Identity 会话快照和 NetworkProfile 分别写入宿主选择目录中的普通 JSON；没有旁置密钥文件。所有保存仍需用户逐次 opt-in，默认只保存在 Client 生命周期内。Rust 在 Unix 设置目录 `0700`、文件 `0600`，这只是权限限制，不是加密；同一 OS 用户进程可直接读取内容。宿主可自行检查、备份和删除相应目录。

统一身份与网络自助仍是仅有的两个 Auth 域。校园网 Portal/Tsinghua Secure 账号可作为本机网络资料保存和显式填写，不参与 Auth 状态、会话快照或恢复链。密码、Cookie 和网络资料都按明文 JSON 处理，因此该功能必须保持显式 opt-in。读取失败、格式错误、权限不合规或旧加密格式均返回存储错误并保留原文件；不静默迁移、删除或伪装成空结果。用户决定清除后可通过明确清除操作删除旧记录；未实现自动解密旧格式。

当前文件名为 Identity 会话快照 `campus-session-v3.json`、凭据记录 `credential-v2-<账号散列>.json`、网络资料 `profiles-v2.json`。App 提供根目录与 namespace，SDK 将各用途分别放置，互不混用。合成数据定向测试通过：凭据 8 项、网络资料 4 项、会话/元数据 15 项；SDK `client_api` 11 项、`public_api` 14 项。`cargo check -p tsinghua_kit_ffi --lib`、严格 Rustdoc、FRB codegen、`flutter analyze lib`、公开入口 Flutter 测试、Rust 格式和 `git diff --check` 均通过。文件明文可读、两个账号域隔离、无旁置密钥文件、旧格式/损坏记录保留并报错、有效但过期的会话快照清理均有覆盖。未执行真实账号登录或任何学校服务请求。

## 51. 2026-09-28 引擎回归归因与三处修复

本轮在 `rust/` 引擎库上做一次失败归因，不做 Flutter 页面迁移。方法是把 `a4db9fb` 基线单独构建（`/private/tmp/base4-target`），在相同环境（`THYOU_SESSION_DIR` 指向新建的绝对目录、`TMPDIR` 隔离、`RUST_MIN_STACK=16777216`）下对同一测试逐项对比，并对疑似新增失败单独串行复跑。**按名次差集的“新增失败”必须先单独复现才计入**：全量并行运行时存在跨测试的文件系统与环境竞争，未复现的差集项不计为回归。

三处真实修复：

1. `thos_compat_error` 把 `ErrorCode::SessionExpired` 重新映射为旧的“网上服务大厅会话已失效，请重新建立登录会话”。typed SDK 的 `SessionExpired` 与“账号会话未确认”的合并仍保留，仅兼容字符串恢复原状。
2. 新增稳定错误码 `ErrorCode::RedirectRefused`（`redirect_refused`），与 `InvalidResponse` 明确区分：响应的跳转目标离开已映射的服务路由时，读取被允许名单拒绝，被测响应本身可能是完全合法的。`map_thos_error` 的 `ThosError::Route` 改映射到该码，兼容字符串恢复“网上服务大厅返回了未允许的地址，已停止读取”。上一轮把 `Route` 并入 `ContextMismatch` 会把“阶段性事项引用已失效”这类真正的上下文不匹配与路由拒绝混为一谈。该错误码为 `#[non_exhaustive]` 枚举的新增项；Flutter 侧 `TsinghuaKitFailure` 仅按 `service`/`code` 匹配，未覆盖的组合落到 `_sharedCodeMessage`，不会崩，后续 SDK 版本应在 `lib/data/tsinghua_kit_failure.dart` 补 `('service_hall','redirect_refused')` 文案。
3. `backend_repair_explicit_invalid_credentials_revoke_saved_metadata_locator` 原用 `CampusRuntime::new` 构造；该构造自持持久化，要求真实应用数据目录存在，在隔离环境里必然以“应用私有数据目录不可用”失败。改为 `new_with_persistence(..., false)`，与基线一致，且该用例只验证内存中的撤销记账。

另外两处与本体无关的收尾：`identity_execution.rs` 的跳转续接循环把 `MAX_IDENTITY_REDIRECT_RETRIES = 0` 的常量改成显式单次发起（该比较被 `clippy::absurd_extreme_comparisons` 判为恒假，是基线已存在的编译错误）；`tsinghua-kit-check` 二进制要求的 `terminal-check` feature 此前只打开了 `rpassword`/`tokio`，没有转发到引擎的 `cli_validation` 模块，`--all-features` 下无法编译，已补 `tsinghua_kit_engine/terminal-check`。

验证：全量串行引擎套件（排除两个基线同样挂起的用例）head 114 项失败，基线三轮并集 131 项；对差集逐项单独复跑后，**没有一项是仅 head 出现的稳定失败**。两个挂起用例 `backend_repair_registrar_only_calendar_reaches_display_dto_without_learn_requests` 与 `backend_repair_runtime_electricity_history_refreshes_business_proof` 在基线二进制上同样挂起，确认为既有缺陷、非本轮引入，仍待修。`cargo clippy --all-targets --all-features -- -D warnings` 由「无法编译」变为可编译并通过 `cargo clippy`（仍有 129 条 warning，未开启 `-D warnings` 作为本轮门禁）；`RUSTDOCFLAGS="-D warnings" cargo doc` 通过。四个 FFI contract 目标（classroom 3、info 4、learn 1、webvpn 2）失败项在基线工作树 `/private/tmp/base4` 上数量完全一致，确认全部为既有失败。未执行任何真实账号登录或学校服务请求。

## 52. 2026-09-28 App 已切换到含 ServiceHallDirectory 导出的公共提交

THYou 侧把 `tsinghua_kit` 的 git `ref` 从 `3eb896c`（`v0.1.1` 之后的一次提交，`lib/service_hall.dart` 尚未导出 `ServiceHallDirectory`）改为 `7d28a58`，即 `origin/main` 当前 HEAD。两者之间只有一项内容差异：

```
lib/service_hall.dart             | 1 +
test/public_entrypoints_test.dart | 1 +
```

因此 App 侧不再需要那段“从 `package:tsinghua_kit/tsinghua_kit.dart` 取同一声明”的临时导入，`lib/data/tsinghua_kit_service_hall_read.dart` 与 `lib/state/tsinghua_kit_thos_controller.dart` 已删除该 workaround，并在 `service_hall.dart` 的别名下拼写 `sdk.ServiceHallDirectory`。

验证：`flutter pub get` 将锁文件解析到 `7d28a58`（版本仍为 `0.2.0-alpha.1`）；`flutter analyze lib test` 无问题；`flutter test test/tsinghua_kit_thos_controller_test.dart test/tsinghua_kit_failure_test.dart` 13 项通过。SDK 侧 `flutter analyze lib test` 无问题，`flutter test test/public_entrypoints_test.dart test/auth_status_test.dart test/second_factor_method_test.dart` 5 项通过，`cargo check --workspace --all-targets` 退出 0。App 侧 `flutter build macos --debug --no-pub` 退出 0（产物 `build/macos/Build/Products/Debug/thyou.app`；仅剩 `tsinghua_kit` 插件尚未支持 Swift Package Manager 的工具级提示）。未执行真实账号登录或学校服务请求。

已发布：`23aefda`（`ErrorCode::RedirectRefused`）与 `10c4ea8` 已随本次提交一起推到 `origin/main`（`7d28a58..e70a9e5`），App 侧 `lib/data/tsinghua_kit_failure.dart` 的 `('service_hall','redirect_refused')` 中文文案现在有对应引擎代码与线上提交；生产路由切换仍未开始，`FrbCampusRuntimeGateway`/`lib/src/rust` 仍是唯一执行真实请求的路径。

## 53. 2026-09-29 零新认证教务域：培养方案完成情况与体测成绩

本轮按已批准的计划补上 `thu_reference` 里已实现、而 TsinghuaKit 缺失的**零新认证只读域**的前两项，两者都复用已有 INFO/WebVPN 会话与已有 zhjw（registrar）映射，不新增认证路径、不新增 Cookie jar、不绕过 `CampusHttpTransport` 与 request gate。

**新增引擎模块**

- `program_read.rs`：培养方案完成情况。`ProgramAdapter` 走 `/jhBks.by_fascjgmxb_gr.do?m=queryFaScjgmx_gr&xsViewFlag=pyfa&…`，解析器把摘要块、`.table-striped` 课程表与方案外课程表分别还原为 `ProgramCompletion` / `CourseSetCompletion` / `CourseCompletion`。参考实现用 `illegalCourseLevelFlag` 表达“属性列不认识”，引擎保留同一语义：**未知属性不是默认任选，而是 `UnknownAttribute` 解析错误**；`W`/`F`/`I` 等标记一律是 `NotCompleted`，不会被当作已完成课程；方案外课程是独立分组（`CourseSetKind::Excluded`），不并入方案内学分。参考库的 `PROGRAM_WEBVPN_TARGET = 287C0C6D90ABB364CD5FDF1495199962` 早已在 `InfoSessionAdapter::map_additional_roaming` 的允许名单内，因此**无需新增 selector 条目**。
- `physical_exam_read.rs`：体测成绩。走 `/tyjx.tyjx_tc_xscjb.do?m=jsonCj`，选择器 `8BF4F9A706589060488B6B6179E462E5` 解析到同一个 zhjw 映射（本轮只把该 selector 登记进允许名单，映射 id 没有增加）。服务自己的 `success === "false"` 是**合法空态**（`no_result`），不是失败，也不是补零记录；非 JSON、缺 `success`、复合字段值都是明确解析错误。
- `campus_html.rs`：两个 HTML/JSON 域共用的页面分类（登录页 / 超时页 / 未知）与有界元素扫描；分类发生在任何解析之前，因此登录失效不会被误判成“格式异常”。

**APP 自动结算分的处理**（本轮唯一需要人工判断的语义）

`PhysicalExamReport::reference_total` 是**在 Rust 内按固定权重本地重算**的参考值，不是服务返回的成绩，也不进任何缓存。它与常量标签 `参考成绩（APP自动结算，仅供参考）` 成对暴露，FFI DTO 与 Dart 都强制携带该标签。规则是：**未参加的项目不计入（其权重贡献为 0，这正是总分能存在的原因——一个学生只跑 800m 或 1000m 之一，不可能两者都有）**，而**服务确实报了分但读不成数字**的情况会让整个参考总分缺席，而不是悄悄变小。

**缓存策略**

两个域都是**只读且不落缓存**：培养方案完成情况与体测成绩都是成绩类文档，一份陈旧副本对调用方没有价值，反而会引入“把旧成绩当当前成绩渲染”的错误类别，所以失败就是失败。

**Runtime 接线（与既有教室/电费域同构）**

`ensure_program_reader_session` / `prepare_program_adapter` 与 `ensure_physical_exam_reader_session` / `ensure_physical_exam_session` 都要求 INFO 会话已证明后才发起 handoff；handoff 返回的 URL 只用于**校验**是否落在 `REGISTRAR_WEBVPN_BASE_URL` 映射根内，随后把 path 收敛到映射根、清空 query/fragment —— handoff 自带的 `ticket` 永远不成为适配器 base URL。业务证明用 `AtomicU64` binding 计数器绑定到适配器实例，`*_service_is_proven()` 同时要求 INFO 已证明且证明与当前实例匹配。`ServiceId::Info` 失效与账号登出都会清除这两个域的状态。

**FFI / Dart**

`sdk_api.rs` 新增 `PhysicalExamResultDto` / `PhysicalExamDataDto` / `PhysicalExamItemDto` 与 `ProgramCompletionResultDto` / `ProgramCompletionDataDto` / `ProgramCourseSetDto` / `ProgramCourseDto`，`Debug` 全部脱敏（只打印计数与“是否有值”标志），课程状态与课组类别在上桥时字符串化；`ClientHandle::physical_exam_result` 与 `ClientHandle::program_completion_result` 为仅有的两个新入口。FRB 2.13.0 重新生成，生成物未手工编辑。Dart 侧新增 `lib/src/physical_exam.dart`、`lib/src/program.dart` 两个 part 文件与对应的 `lib/physical_exam.dart`、`lib/program.dart` 入口；`ProgramCourseState` / `ProgramCourseSetKind` 是手写枚举，未知字符串落到保守取值而不是抛错。

**验证**

`cargo test -p tsinghua_kit_engine --lib program_tests` 14 项通过（另有 `api::runtime::program_tests` 3 项通过：handoff 后的读取落在映射根、handoff 的 ticket 不出现在后续请求、失效页返回失败而不是旧报告、账号未证明时零请求）；体测定向 `physical_exam_tests` 16 项通过。`cargo check --workspace --all-targets` 退出 0；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`cargo fmt --all -- --check` 与 `flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖两个新入口。`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新（`source_revision`、模块列表、根 `pub use` 计数、DTO/方法清单、`direct_state_field_count`）。**未执行任何真实账号登录或学校服务请求**；这两个域的线上可用性仍未验证。

## 54. 2026-09-29 零新认证教务域：教学评估问卷列表

本轮补上 `thu_reference` 已实现、TsinghuaKit 缺失的**教学评估问卷列表**（`jxgl.cic.tsinghua.edu.cn`）。它不属于原有 zhjw（registrar）映射，因此本轮**新增了一个 selector 允许名单条目**——这是本轮唯一新增的映射。

**新增引擎模块 `assessment_read.rs`**

- 路径 `/jxpg/f/jxpg/wj/xs/pgkcList`，module 常量只保存 path，不保存 URL；`ASSESSMENT_WEBVPN_TARGET = 0D8B99BA23FD2BA22428D9C8AA0AB508` 是参考实现的 roam selector，映射 id 为 `77726476706e69737468656265737421faef469069336153301c9aa596522b20e33c1eb39606919f`（host `jxgl.cic.tsinghua.edu.cn`，scheme `http`）。`InfoSessionAdapter::map_additional_roaming` 新增该 selector → `(host, "http", mapping)` 三元组，是所有新域的唯一扩展点；未登记的 selector 一律 `UnexpectedOrigin`/`HandoffUnexpectedPath`，因此本域仍然只经由既有 transport、request gate 与账号绑定，没有第二套请求路径。
- `AssessmentAdapter` 与既有 `program_read`/`physical_exam_read` 同构：`*_RequestPlan` + `*Profile` 常量、`AtomicU64` binding 计数器绑定的 `AssessmentBusinessProof`、`try_with_transport(base_url, transport)` 共享同一个 identity Cookie jar、`execute()` 在解析之前先分类 login/expiry/origin/path/content-type、无 body 的 `AssessmentAdapterError`（带 `diagnostic_code()` 与 `is_session_expired()`）。

**本域唯一需要判断的语义：位置索引 vs. 结构锚点**

参考实现按列号取值（第 5 列课程名、第 9 列是否已评、第 11 列 `onclick` 里的表单路由）。引擎的 `campus_html` 刻意**不提供**位置索引能力，所以 `assessment_read.rs` 自己承担这个决定并为之付出代价：**每一行都必须同时携带内联的 `Body('…')` 调用，并且该路由必须能作为同一映射下的合法相对路径通过校验**。列序一旦漂移，行为 `UnrecognizedRow`/`MissingAction`/`InvalidRoute` 失败，而不会把某一列的文本当成另一列的语义上报。因此解析器接受的行是"结构上可锚定的行"，不是"列数够多的行"。

**两种不同的非正常态被显式区分，而不是塌缩成空结果**

- 问卷窗口未开：服务返回 200 加自己的提示 `对不起，现在不是填写问卷时间` ⇒ `AssessmentParseError::NotOpen` ⇒ `AssessmentAdapterError::NotOpen` ⇒ 上桥后 `diagnostic_code() == "assessment_not_open"` ⇒ SDK 层映射到**本轮新增的 `ErrorCode::NotAvailable`**（`as_str() == "not_available"`）。`NotAvailable` 与 `ServiceUnavailable`（服务坏了）、`CacheMiss`（本地没有副本）是三个不同的稳定状态，调用方不需要读任何文案就能区分"现在没得做"和"这次没读成"。
- 列表为空：`EmptyList` 是**失败**，不是"校验通过的空列表"。一个 200 + 空表格既可能是窗口刚关，也可能是权限变更，引擎不替它选一个。

**表单路由不离开 Rust**

每行的表单路由由 `AssessmentRef`（adapter binding + generation + index）间接持有，路由表放在 adapter 内的 `Mutex<AssessmentRoutes>`。`generation` 只在**一次成功解析之后**才前进，因此一次被拒绝的读取不会作废先前已发出的引用；而 `form_path()` 会拒绝外来 binding 与过期 generation。adapter 被丢弃（INFO 会话失效、登出）时路由表随之消失，早先发出的引用不再可解，且 `AssessmentRef` 的 `Debug` 只打印 index。

**Runtime 接线**

`ensure_assessment_reader_session` / `prepare_assessment_adapter` 要求 INFO 已证明后才发起 handoff；handoff 返回的 URL 只用于**校验**是否落在新的 `ASSESSMENT_WEBVPN_BASE_URL` 映射根内（否则 `assessment_mapping_rejected`），随后把 path 收敛到映射根、清空 query 与 fragment —— handoff 自带的 `ticket` 永远不成为适配器 base URL。`load_assessment_list_result` 带一次性过期恢复，失败时把 `diagnostic_code()` 记进 `last_assessment_failure_code`（供 SDK 层做稳定错误分类）再 `record_business_failure("assessment", "assessment_list", …)`；成功时清空该记录。`assessment_service_is_proven()` 同时要求 INFO 已证明且证明与当前 adapter 实例匹配；`ServiceId::Info` 失效与 `logout` 都会清除 adapter 与 proof。

**FFI / Dart**

`sdk_api.rs` 新增 `AssessmentListResultDto` / `AssessmentListDataDto` / `AssessmentListItemDto`，`Debug` 脱敏为条目数 + 已评数；`reference_index` 是 Rust 会话内的索引，不是服务路由。`ClientHandle::assessment_list_result` 是唯一新入口。FRB 2.13.0 重新生成，生成物未手工编辑。Dart 侧新增 `lib/src/assessment.dart` part 文件与 `lib/assessment.dart` 入口；`AssessmentItem.referenceIndex` 的文档明确规定它不可伪造、且随会话失效而失效。

**验证**

`cargo test -p tsinghua_kit_engine --lib assessment` 20 项通过（`assessment_tests` 15 项 + `api::runtime::assessment_tests` 5 项）：位置读取与结构锚点、空列表是失败、窗口未开是独立类别、登录页/超时页是会话失败、无内联动作的行被拒、越界路由被拒、过短行被拒、缺 `tbody` 被拒、adapter 走 cookie-aware transport、引用只在产生它的 adapter 内可解、被取代的列表引用不再可解、窗口未开以 `assessment_not_open` 到达调用方且 `route_generation() == 0`、解析失败上报 `assessment_list_empty`、非 HTML 响应上报 `assessment_content_type`、adapter `Debug` 不含路由或映射 token。Runtime 4 项 fixture 断言读取请求落在映射根内且不含 handoff 的 `ticket`。`cargo check --workspace --all-targets` 退出 0；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖新入口。`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新。**未执行任何真实账号登录或学校服务请求**；本域的线上可用性仍未验证。

## 55. 2026-09-29 零新认证生活域：发票列表与报销凭证 PDF

本轮补上 `thu_reference` 已实现、TsinghuaKit 缺失的**电子发票列表与文档**（`dzpj.tsinghua.edu.cn`）。它与 §54 的教学评估域一样不属于原有 zhjw 映射，因此本轮**又新增了一个 selector 允许名单条目**，其余一切沿用已有 INFO/WebVPN 会话、已有 transport 与 request gate。

**新增引擎模块 `invoice_read.rs`**

- 路径常量只保存 path：`INVOICE_LIST_PATH = /invoiceSys/getList.do`、`INVOICE_DOCUMENT_PATH = /invoice/showInvPdf.do`、`INVOICE_ROAM_AUTH_PATH = /roam/roamAuth.do`。`INVOICE_WEBVPN_TARGET = 625B81A7A9D148B01DA59185CC4074E1` 是参考实现的 roam selector，映射 id 为 `77726476706e69737468656265737421f4ed519669247b59700f81b9991b2631aee63c51`（host `dzpj.tsinghua.edu.cn`，scheme `https`）。`InfoSessionAdapter::map_additional_roaming` 新增该 selector → `(host, "https", mapping)` 三元组。
- `InvoiceAdapter` 与 `program_read`/`physical_exam_read`/`assessment_read` 同构：`InvoiceRequestPlan` + `InvoiceProfile` 常量、`AtomicU64` binding 计数器绑定的 `InvoiceBusinessProof`、`try_with_transport(base_url, transport)` 共享 identity Cookie jar、`execute()` 在解析之前先分类 login/expiry/origin/path/content-type、无 body 的 `InvoiceAdapterError`（带 `diagnostic_code()` 与 `is_session_expired()`）。`InvoiceProfile` 只会发出 GET 与一个固定表单体的 POST，写路由无法被夹带进来。
- 列表表单固定为 `page`、`limit=20`、`columnName=inv_date`、`sort=desc`；分页上界由 `MAX_INVOICE_PAGE = 1000` 在**发请求之前**判定，越界直接 `PageOutOfRange`。

**本域唯一需要判断的语义：两跳一次性 ticket handoff**

发票应用的 handoff 不是普通的 roam 跳转，而是**两次交换**：先 GET roam 目标页，从内联脚本 `("ticket").value = '…'` 里取出一次性 ticket，再把它 POST 回 `/roam/roamAuth.do`，只有第二跳的最终 URL 才算拿到了可用的映射根。因此：

- ticket 的提取要求锚点、赋值号与两侧引号**按序出现**，并带长度上下界（8–512），所以被改写或截断的赋值只会被报成"没有 ticket"，而不会从无关文本里切出一段冒充 ticket。
- 两跳都只接受**同一 origin 且仍在同一映射前缀内**的 URL，带 `%`、`\`、用户名、密码或 fragment 都拒绝。
- ticket 在 POST 之后即被丢弃；它**不成为适配器 base URL**，也不会出现在任何后续请求或 DTO 里。`prepare_invoice_adapter` 只把 handoff 结果用于**校验**是否落在 `INVOICE_WEBVPN_BASE_URL` 映射根内（否则 `invoice_mapping_rejected`），随后把 path 收敛到映射根、清空 query 与 fragment。
- 第二跳的**结果不明确**（传输失败、body 读不成）是独立的 `InvoiceHandoffError::Unconfirmed`，它**不会**触发重放：一次性 ticket 已经被消费，重放一个语义不明的 ticket 交换正是 AGENTS.md 禁止的那类自动重试。

**金额用精确整数分，不用浮点**

服务返回的是两位小数字符串。`exact_cents` 只在 token 真的是整体十进制（允许前导 `+`/`-`、最多两位小数、无指数、无多余字符）时换算成 `i64` 分，任何有损表示都报 `InvalidAmount`，因此调用方拿到的分与服务的分逐位相等，不会出现 `0.1 + 0.2` 式的漂移。

**服务自己的记录标识不离开 Rust**

`InvoiceRef`（adapter binding + generation + index）间接持有文档标识，标识表放在 adapter 内的私有状态里。`generation` 只在**一次成功解析之后**才前进，所以一次被拒绝的读取不会作废先前已发出的引用；`document_id()` 会拒绝外来 binding 与过期 generation。adapter 被丢弃（INFO 会话失效、登出）时标识表随之消失。`InvoiceRef` 的 `Debug` 只打印 index。

**响应类型必须自证，不能"大概像"**

文档读取只接受**真的**是那份 PDF 的响应：`%PDF-` magic 与 `MAX_DOCUMENT_BYTES = 12 MiB` 双重约束。一个 200 但内容不是 PDF 的响应（典型的是又一个登录页或错误页）报 `invoice_template`，而不是把 HTML 字节当成凭证交给调用方。列表同理：解析失败（非 JSON、缺 `data`、缺 `count`、行字段缺失、行数超 `MAX_ROWS`）一律是**失败**，不会被塌缩成"这个账号没有发票"的空页——空页只有在响应确实带着一个格式良好的记录数组时才产生。

**Runtime 接线**

`ensure_invoice_reader_session` / `prepare_invoice_adapter` 要求 INFO 已证明后才发起 handoff。`load_invoice_list_result` / `load_invoice_document_result` 各带一次性过期恢复，失败时把 `diagnostic_code()` 记进 `last_invoice_failure_code`（供 SDK 层做稳定错误分类）再 `record_business_failure("invoice", "invoice_list"|"invoice_document", …)`；成功时清空该记录。`invoice_service_is_proven()` 同时要求 INFO 已证明且证明与当前 adapter 实例匹配；`ServiceId::Info` 失效与 `logout` 都会清除 adapter 与 proof。两个域都不落缓存：报销状态是可变财务状态，一份陈旧副本会被渲染成当前状态。

**FFI / Dart**

`sdk_api.rs` 新增 `InvoiceListResultDto` / `InvoiceListDataDto` / `InvoiceRecordDto` / `InvoiceDocumentResultDto` / `InvoiceDocumentDataDto`，`Debug` 脱敏为记录数、总数与字节数。文档引用沿用 `LibraryRef` 的既有做法：`ClientHandle` 持一个 `HashMap<String, InvoiceRef>`，对每行发出一个新的 UUID 作为 `reference_id`，引擎的 `InvoiceRef::index` **不出现在公开 surface 上**；未知 id 报 `context_mismatch("invoice")`。FRB 2.13.0 重新生成，生成物未手工编辑。Dart 侧新增 `lib/src/invoice.dart` part 文件与 `lib/invoice.dart` 入口，金额与总数的 `PlatformInt64` 经既有 `_platformInt64ToBigInt` 转换，因此 Dart 侧是 `BigInt` 而不是可能丢精度的 `int`。

**验证**

`cargo test -p tsinghua_kit_engine --lib invoice` 40 项通过（`invoice_tests` 23 项 + `api::runtime::invoice_tests` 8 项，其余为过滤统计）：两跳 handoff 的 URL 与映射约束、缺 ticket 的 roam 页被拒、登录页是会话失败、ticket 不出现在后续请求、列表按精确响应形状解析、空数组是合法空页而解析失败不是空页、越界页在任何列表请求之前被拒、文档读取有界且可解引用、非 PDF 的 200 响应报 `invoice_template`、过期页是会话失败、账号未证明时零请求、引用不跨会话存活、adapter `Debug` 不含标识或映射 token。相邻域同轮复跑：`assessment` 20 项、`program_tests` 17 项、`physical_exam` 16 项通过。`cargo check --workspace --all-targets` 退出 0；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`cargo fmt --all -- --check` 与 `git diff --check` 干净；`flutter analyze lib` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `InvoiceClient`/`InvoicePage`/`InvoiceRecord`/`InvoiceDocument` 与 `maxPage == 1000`、`pageSize == 20`。`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新。

**顺带修掉的测试基础设施缺陷**

本轮新增的 fixture 测试暴露出 loopback 夹具一个**既有**的间歇性失败（约 1/7–1/10）：`reference_test_support::FixtureServer` 的 `accept()` 在 BSD 系平台上会继承 listener 的非阻塞标志，于是 `read_request` 在客户端字节到达前就返回 `WouldBlock`，夹具于是对半读的请求作答，调用方看到 `TransportError::Request(..)` 与 `received unexpected message from connection`。已在 accept 之后显式 `set_nonblocking(false)`，让读超时而不是非阻塞标志来约束这次读取。修复后发票 40 项、评估 20 项连续复跑全绿（此前评估的 `a_closed_window_reaches_the_caller_as_not_open` 在同样条件下会偶发失败）。这是夹具的修复，不是对被测代码的放宽。

**边界**

本轮**未执行任何真实账号登录或学校服务请求**；本域的线上可用性仍未验证，需另行真实只读验收。发票域只读，不含任何报销或支付动作；`thu_reference` 侧的相关实现仅作为路径/字段/选择器/可观察行为的证据使用，未复制其源码、夹具或资源。

## 56. 2026-09-29 零新认证生活域：银行到款（含基金会）与研究生收入

本轮补上 `thu_reference` 已实现、TsinghuaKit 缺失的**银行代发到款**（`yhdf.tsinghua.edu.cn`，`/yhdfcx/search.do` 与 `/yhdfcx_jjh/search.do` 两个路径族）与**研究生收入**（`zzjl.graduate.tsinghua.edu.cn`，`/b/yjsjzxt/v_yjszzjl_yjscwdfmx_cx/pageList`）。两者都是**钱款对账单**，因此共用同一套精确金额与有界读取规则，但各自有独立的 selector、映射与适配器实例。

**新增引擎模块 `bank_read.rs` 与 `money.rs`**

- `money.rs`（crate-private）把两位小数字符串换算成 `i64` 分：`exact_cents` 只接受整体十进制 token（允许前导 `+`/`-`、最多两位小数、无指数、无多余字符），`json_cents` 同时接受字符串与 JSON 数字但同样拒绝有损表示。任何有损值报 `AmountError`，因此调用方拿到的分与服务的分逐位相等。第三条规则是**空列不是零**：服务在某个金额列什么都没印时，该字段是 `None`，而不是 `Some(0)`。
- `bank_read.rs`：`BankPaymentProfile` 持常量请求计划，`BankPaymentRequestPlan` 只保存 path 与要提交的年份，绝不保存 URL；`BankReceiptRow` / `BankReceiptMonth` / `BankPaymentLedger` 是解析结果。`BankPaymentAdapter` / `GraduateIncomeAdapter` 与既有 `invoice_read`/`assessment_read` 同构：`AtomicU64` binding 计数器绑定的 `*BusinessProof`、`try_with_transport(base_url, transport)` 共享同一个 identity Cookie jar、`execute()` 在解析之前先分类 login/expiry/origin/path/content-type、无 body 的 `*AdapterError`（带 `diagnostic_code()` 与 `is_session_expired()`）。

**本域唯一需要判断的语义：参考实现的三路并发改为有界串行派发**

参考客户端在这里用 `Promise.all` 一次并发三个年份批次。引擎侧**不能**照搬：一次调用若同时把三、四个请求推上线路，就绕过了统一门禁 `MAX_READ_DISPATCHES = 4` 的意图，也会让 `request_gate` 的退避形同虚设。因此 `read_ledger()` 把年份按 `MAX_YEARS_PER_BATCH = 4` 切分后**顺序**派发，批次上界 `MAX_YEAR_BATCHES = 24`、年份上界 `MAX_YEARS = 96`，一次成功读取最多 24 个批次，每个批次都单独经过 `CampusHttpTransport` 与 request gate。这是引擎侧主动的性能取舍，不是对参考实现的忠实复刻，模块文档与测试注释里都写明了原因。

**年份表单是全部后续请求的边界**

第一个响应的 `<option>` 集是服务自己给出的、该账号有到款记录的年份，因此**没有任何年份是凭空发明的**：调用方无法索取服务从未提供的年份。`NoYears`（空 option 集）是**失败**，不是"这个账号没有到款"——200 加一个空表单既可能是账号确实无记录，也可能是页面改版，引擎不替它选一个。年份值与表单再编码逐字节复核（`year=2021&year=2020`；敌意值 `"2&year=9"` 变成 `year=2%26year%3D9`），所以调用方文本永远无法拼进表单体。

**按表头标签读数，不按列号**

代发表格的每一列都由 **header 标签**定位（`代发部门`/`代发项目`/…/`应发金额`/`扣税金额`/`实发金额`/`存折金额`/`现金金额`），列序漂移只会报"表头不符"，不会把某一列的数字当成另一列的语义。这与 §54 的教学评估域同一原则：`campus_html` 刻意**不提供**位置索引能力。测试里有一条"列序调换后按标签取值"的用例以及两条 `SectionCountMismatch`（表头 1 / 表 0、表头 1 / 表 2），后者保证"标题与表格必须一一配对"。

**基金会账本是与主账本同主机同映射的另一条路径**

参考客户端的两个 search URL 只有 `/yhdfcx` 与 `/yhdfcx_jjh` 之别，映射 token 完全相同，因此两条 selector 登记到**同一个** `(host, scheme, mapping)` 三元组，由请求计划区分账本。`BankLedger::{Main, Foundation}` 在 SDK 与 FFI 上都是显式枚举，`ensure_bank_payment_reader_session` 只在**已证明且账本相同**时复用适配器，切账本会重新走上 handoff。

**host 名标注为推断**

`info_session.rs` 的两条新允许名单臂分别写作 `yhdf.tsinghua.edu.cn`（主/基金会）与 `zzjl.graduate.tsinghua.edu.cn`（研究生收入）。这两个主机名是从参考实现的主机表**推断**出来的，不是从任何一次真实响应中观察到的，代码注释里明确写了 `inferred, not evidenced`。映射 token 本身来自参考实现的选择器表，是可用证据。

**研究生收入：越界参数在会话工作之前就被拒绝**

`load_graduate_income_result(begin, end)` 在 `ensure_identity_user_for_live_read` 之前先跑 `GraduateIncomeProfile::standard().list_request(begin, end)`：两个边界都必须是八位数字，否则记下 `last_graduate_income_failure_code = "graduate_income_range"` 并在**零请求**的情况下返回失败。理由写在代码里：一个非法参数不该花掉一次 handoff，其自由文本也永远不该到达服务端。`rows` 固定上界 1000，`total` 是可选总数；**缺 `rows` 数组是失败**（`MissingRows`），而显式空数组才是服务自己的"无收入"答复。

**Runtime 接线**

`ensure_bank_payment_reader_session` / `prepare_bank_payment_adapter`（及研究生收入的同构方法）要求 INFO 已证明后才发起 handoff；handoff 返回的 URL 只用于**校验**是否落在 `BANK_WEBVPN_BASE_URL` / `GRADUATE_INCOME_WEBVPN_BASE_URL` 映射根内（否则 `bank_mapping_rejected`），随后把 path 收敛到映射根、清空 query 与 fragment —— handoff 自带的 `ticket` 永远不成为适配器 base URL。两个 load 各带一次性过期恢复（失效 → 刷新 INFO → 重新 handoff → 重读，第二次仍过期才 `fail("银行到款自动续接后仍已过期，请重新建立")`），失败时把 `diagnostic_code()` 记进 `last_bank_payment_failure_code` / `last_graduate_income_failure_code` 再 `record_business_failure(...)`。`bank_payment_service_is_proven()` / `graduate_income_service_is_proven()` 同时要求 INFO 已证明且证明与当前 adapter 实例匹配；`ServiceId::Info` 失效与 `logout` 都会清除 adapter 与 proof。两个域都不落缓存：到款状态是可变财务状态，一份陈旧副本会被渲染成当前状态。

**SDK 错误分类**

`error_sdk.rs` 新增 `Service::BankPayment` / `Service::GraduateIncome`；SDK 的 `bank_payment_failure` / `graduate_income_failure` 把 `bank_auth_required` / `graduate_income_auth_required` 映到 `SessionExpired`，`bank_years_empty` / `bank_months_empty` 映到 `NotAvailable`，`graduate_income_range` 映到 `InvalidInput`，`*_size` 映到 `IncompleteResult`，`*_network` 映到 `NetworkUnavailable`，`*_origin` / `*_path` 映到 `RedirectRefused`，`*_http` 映到 `ServiceUnavailable`，其余落到 `InvalidResponse`，最后回退到 identity auth 状态。

**FFI / Dart**

`sdk_api.rs` 新增 `BankReceiptDto` / `BankReceiptMonthDto` / `BankPaymentLedgerDataDto` / `BankPaymentLedgerResultDto` / `GraduateIncomeRecordDto` / `GraduateIncomeDataDto` / `GraduateIncomeResultDto` 与 `BankLedgerDto`（`Main`/`Foundation`），`Debug` 全部脱敏（只打印 `month_count` / `receipt_count` / `record_count` / `total`）。金额跨桥一律是 `PlatformInt64` 分；Dart 侧新增 `lib/src/bank.dart` part 文件与 `lib/bank.dart` 入口，用 `_optionalInt64ToBigInt` 保留"服务没印"与"零金额"的区别（`null` vs `BigInt.zero`），`receiptCount` 与 `total` 同样经既有 `_platformInt64ToBigInt` 转换，因此 Dart 侧是 `BigInt` 而不是可能丢精度的 `int`。FRB 2.13.0 重新生成，生成物未手工编辑。

**验证**

`cargo test -p tsinghua_kit_engine --lib -- bank_tests` **28 项通过**（`bank_tests` 20 项 + `api::runtime::bank_tests` 8 项）：年份表单解析与空 option 失败、按表头标签读数（含列序调换）、`SectionCountMismatch` 两例、非精确金额被拒、登录页/超时页是会话失败、表单再编码（含敌意值）、基金会账本的独立 path 与 selector、收入区间两端必须是八位、adapter `Debug` 不含映射 token、映射根之外的 base URL 被拒、批次**顺序**派发（断言 `GET` 后 `POST`，且 body 为 `year=2021&year=2020`）、收入行精确分与请求 query（`ffkssj=20260101&ffjssj=20261231&rows=1000&page=1&sidx=id&sord=asc`）、被拒区间**零请求**、缺 `rows` 是失败而空数组是合法空页、缺 id 行被拒、非精确收入金额被拒；Runtime 8 项覆盖：INFO 会话内读取（5 请求，含映射根路径与批次体）、基金会账本作为同映射的另一条路径、切账本重新证明（10 请求）、`bank_years_empty` 失败且无后续到款请求、过期页 ⇒ `!bank_payment_service_is_proven()`、账号未证明时两次读取均零请求失败、研究生收入走自己的映射（`requests[3]` 断言路径与区间参数）且只证明自己、非数字区间以 `graduate_income_range` 到达调用方并在**发请求之前**被拒（`graduate_income_adapter.is_none()`，INFO 仍证明）。

`cargo check --workspace --all-targets` 退出 0；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`cargo fmt --all -- --check` 与 `git diff --check` 干净；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `BankClient`/`BankPaymentLedger`/`BankReceiptMonth`/`BankReceipt`/`GraduateIncomePage`/`GraduateIncomeRecord` 与 `BankLedger.main`/`foundation`。`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新（`source_revision`、模块列表新增 `bank_read`、根 `pub use` 计数 50、DTO/方法清单、`direct_state_field_count`、渲染项计数）。

**边界**

本轮**未执行任何真实账号登录或学校服务请求**；两个域的线上可用性仍未验证，需另行真实只读验收。两个主机名是推断而非证据。到款域只读，不含任何支付、充值或退订动作；`thu_reference` 侧的相关实现仅作为路径/字段/选择器/可观察行为的证据使用，未复制其源码、夹具或资源。

## 57. 2026-09-29 网上服务大厅按课程号查成绩

`thu_reference` 的 `getScoreByCourseId` 走服务大厅自己的预设查询：`POST /fp/fp/Uniformcommon/selectOnePresetData`，体为 `{"presetKey":"103765749452800","param":{"XH":<学号>,"KCH":<课程号>}}`，映射 token 与既有的 thos 域**完全相同**（`56B13DDF68BB3DEA13D98E1E3E776D3E`），因此本轮**不需要**新增 selector、映射或主机名：新域落在既有的 `thos.rs` 允许名单、既有 transport、既有 request gate 与既有账号绑定之内。

**学号必须由 Rust 派生，绝不接受调用方传入**

参考实现把 `helper.userId` 直接放进请求体。引擎侧不能照搬：`ThosClient::course_score(course_id, student_id)` 的 **course 号** 是调用方参数并先经 `course_id_for_request` 校验，**student 号** 只能由 `api::runtime_thos::read_course_score` 从已证明的 `UserIdentity` 取（`user.username.clone()`），并且在进入 body 之前还要过 `student_id_for_request`（纯数字，长度有界）。因此学号不会成为 SDK/FFI/Dart 的任何参数、任何返回字段、任何日志字段；`ThosCourseScoreDto` 与 `CourseScore` / `CourseScoreDto` 的 `Debug` 都只打印字段的存在性与形状（`name_present` / `grade_present` / `credit`），不打印课程名与成绩原文。

**"服务说没有"与"解析失败"必须分开**

参考实现只做 `JSON.parse` 后取三个键。引擎侧新增 crate-private `course_score.rs`：

- 三个字段全部缺失（或为 `null`、或为空串）⇒ `CourseScore::is_empty() == true`，这是**服务自己关于该账号该课程的陈述**，是合法的空结果；
- 字段存在但格式不对（`XF` 不是可解析的小数、越界、是布尔/对象/数组，`KCMC`/`DJZCJ` 不是字符串）⇒ `CourseScoreError::Response`，**绝不**降级成空结果。

`XF` 走 `is_plausible_credit`（有限且在 `0.0..=100.0`），字符串与 JSON 数字都接受但空串是"没印"而不是零。`from_parts` 内部派生 `empty`，所以调用方无法一边带值一边宣称空。

**读 POST 不重放、登录页是会话失败**

`parse_body` 现在同时用共享的 `campus_html::classify_page` 判定：`您即将登陆/清华大学WebVPN`（`PageClass::Login`）与 `用户登陆超时或访问内容不存在`（`PageClass::Expired`）都归为 `ThosError::Session`，与既有的 `login_message` / `<title>登录</title>` / `i_user` / `/do/off/ui/auth/login` 判定并列；其余 HTML 仍是 `ThosError::Response`，不会变成"空成绩"。读取仍走 `execute_once`，302/307 一律 `ThosError::Route`，绝不把读 POST 重放到重定向目标。

**新的 Service 边界**

`error_sdk.rs` 新增 `Service::CourseScore`（`as_str() == "course_score"`）。不复用 `ServiceHall` 是因为课程号是**调用方参数**：`ServiceHall` 的 `invalid_input` 在 App 投影里是"阶段性事项引用已失效，请刷新列表后重试"，用在这里会是一句假话。`course_score_failure` 把 `course_score_input` 映到 `InvalidInput`、`course_score_auth_required` 映到 `SessionExpired`、`course_score_network` 映到 `NetworkUnavailable`、`course_score_route` 映到 `RedirectRefused`、`course_score_http` 映到 `ServiceUnavailable`，其余落到 `InvalidResponse`，并在无诊断码时回退到 identity auth 状态。

**无缓存**

成绩是可变学术数据，`load_course_score_result` 每次实读，`cached_read_metadata` 以 `ReadSource::Live` 构造元数据，不存在"缓存副本被当成当前成绩"的路径。

**FFI / Dart**

`sdk_api.rs` 新增 `CourseScoreDto` / `CourseScoreResultDto`（脱敏 `Debug`）与 `ClientHandle::course_score_result(course_id: String)`；FRB 2.13.0 重新生成，生成物未手工编辑。Dart 侧新增 `lib/src/course_score.dart` part 文件与 `lib/course_score.dart` 入口（导出 `CourseScore` / `CourseScoreClient` / `ReadResult` / `TsinghuaKitClient`），`TsinghuaKitClient.courseScore` 暴露该 facade，`test/public_entrypoints_test.dart` 增补对应断言。

**验证**

`cargo test -p tsinghua_kit_engine --lib course_score` **11 项通过**（`course_score_tests` 7 项 + `api::runtime::course_score_tests` 4 项）：pinned preset path 与 `presetKey`/`param.XH`/`param.KCH` 的请求体逐字段断言、`Debug` 不含课程名/成绩/学号/课程号、三字段全缺是合法空结果而"有课名无成绩"不是空、格式错误字段与越界学分被拒、课程号与学号两侧的边界（空串、空格、斜杠、`..`、`<script>`、超长、全角数字）被拒、**坏参数在任何请求之前**即以 `course_score_input` 返回且零请求、登录页/WebVPN 标题/超时页/`code:401` 归为 `Session`、非预期 HTML 与坏 `XF` 仍是失败、读 POST 遇 302 报 `Route` 且只发一次；Runtime 4 项覆盖：读取落在既有 thos 映射根内且 body 的 `XH` 等于绑定账号（并断言 DTO 与 `Debug` 都不含学号）、拒绝的课程号零请求、非数字登录名以 `course_score_auth_required` 到达调用方且预设查询**从未发出**、空结果是 `empty == true` 而 HTML/`code:401` 分别报 `course_score_response` / `course_score_auth_required` 且只做一次 pinned handoff。已通过的 `thos` 定向 17 项与 CLI 7 项同轮复跑通过。

`cargo check --manifest-path rust/Cargo.toml -p tsinghua_kit_engine --lib` 与 `-p tsinghua_kit --lib`、`-p tsinghua_kit_ffi --lib --features ffi-bridge` 退出 0；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `CourseScoreClient` / `CourseScore`。`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新（`source_revision`、根 `pub use` 计数 51、DTO 清单新增 `ThosCourseScoreDto`、`direct_state_field_count`、渲染项计数）。

**边界**

本轮**未执行任何真实账号登录或学校服务请求**；本域线上可用性仍未验证，课程预设查询的真实响应形状（字段名与是否分页）只有参考实现作为证据。三个映射与主机名沿用既有 thos 域，未新增任何 selector。资金、支付、退订、挂失类动作不在**本节的只读**范围内：它们在本计划的写操作小节（§59、§63、§64、§65、§68）里单独立项，且不进入只读验收。

## 58. 2026-09-29 宿舍卫生分（卫生成绩）的封闭结论

`HEALTH_WEBVPN_TARGET`（`0a993de7e533cd43a594459abdcab27d/0`）此前记录为"证据不足、故意不实现"。本轮对 `thu_reference` 做了定向侦察，把结论从"类型未知"收紧为"类型已知、且因此更不该实现"，并补齐了阻断原因：

- **第二个响应是图片，不是可解析的序列。** 参考库中该 selector 的唯一消费者把它渲染成图片数据 URL；该库的共享 `uFetch` 只在响应 `Content-Type` 以 `image/` 开头时把 body 当 base64 返回（`application/octet-stream`、`application/pdf` 亦然），其余按文本解码；参考自带的该操作 fixture 是一张图表的 JPEG（635×320），不是数值列表。参考库里的"卫生成绩"页面因此是把分数画成像素后放大浏览，API 层面并不存在分数数组。
- **真正阻断实现的是路由，不是类型。** 图表页与电费页**不在同一个 WebVPN 映射**下（图表 `fdb94c85…` 且为 `https`，电费 `fdee4993…` 且为 `http`），第二跳 URL 又是从第一页图表元素的 `src` 里读出来的而非常量（该元素形状未观测）。该 selector 经 per-app Identity roam 到达，而 roam 的 broker 腿以**明文 host** 指定目的地；参考库只通过另一个项目较早的常量把它和移动端 `myhome` 主机名配对，把该主机名套进 `runtime_electricity_auth.rs` 里已验证的 flow 会是对安全边界的一次猜测。

因此本轮**不新增任何实现**：不新增路径常量、不新增 `Service` 变体、不新增 adapter、不注册 selector、不加 Dart 入口。只更新了 `dorm_electricity_read.rs` 中该常量的文档注释，使其如实陈述"图片而非序列"这一已确认事实与三条阻断证据，避免后来者据"证据不足"这一旧措辞误以为补一次抓包就能实现。

需要真实只读验收才可能推进的动作：用真实账号观测一次该 selector 的第一跳页面（拿到图表元素的真实 `src` 形状与主机名/协议），判定是否值得为一个"只能以图片形式呈现"的能力开放第二条映射。**在此之前不得**用空结果、占位序列或前端自绘图表伪装成已实现。

本轮验证：`cargo fmt --check` 与 `git diff --check` 通过；本小节只改注释与文档，未改动任何可执行代码，因此未重跑引擎测试。线上可用性未验证。

## 59. 2026-09-29 教学评估提交（一次性写操作）

本轮在 §54 的**只读问卷列表**之上补上 `thu_reference` 已有、TsinghuaKit 缺失的两项写能力：读取某一行问卷的表单（`read_form`）与提交填好的问卷（`submit_form`）。两者与 §54 共用同一个 selector `0D8B99BA23FD2BA22428D9C8AA0AB508` 与同一个 jxgl WebVPN 映射，因此**本轮不新增任何 selector、映射或主机名**，全部落在既有 INFO/WebVPN 会话、既有 `CampusHttpTransport` 与既有 request gate 内。这是本仓库第一个**非只读**学校服务调用，所以本节的重点是"写操作凭什么可以安全暴露"，而不是路径。

**不可伪造的提交体是本域的核心设计**

参考实现让调用方把整张表单（含隐藏字段）发回去。引擎侧不能照搬：那等于把事务状态与提交路由一起交给调用方。于是把"事务状态"与"显示副本"拆成两个类型：

- `AssessmentForm`（引擎内部）由解析器 `parse_assessment_form_html` **唯一**产生，携带服务自己渲染的隐藏 name/value 对（含一次性事务标识）。它的字段是私有的，`Clone` 只复制答案而不复制提交权。
- `AssessmentFormView` 是给调用方的**显示副本**，只带课程名、当前分数与评语、每位教师/助教的每道题的题干与当前答案、以及 `field_count`。它**不带**事务字段，也不带任何可以提交回去的 body。
- 调用方据此构造 `AssessmentAnswers`（`AssessmentAnswers::new(reference, score)` + `set_score` / `set_suggestion` / `set_people`），Rust 侧再通过 crate-private 的 `AssessmentAnswers::apply_to` 把答案**写到 runtime 自己持有的那张表单上**。因此 POST 出去的那个 body 永远是"服务自己的页面 + 调用方填的分数与评语"，构造不出第二个 body。

**逐位置匹配，不是"没提到就跳过"**

`apply_person_answers` 要求人数与题数**逐位置完全相等**，否则 `AssessmentInputError::UnknownQuestion`。理由写在代码注释里："我没提到的答案"和"我想保持原样的答案"不是同一个请求——把少送的人静默留在服务原值上，会让一份按另一张问卷构造的答案集半途生效。`comment` 为 `None` 才是"保持服务原值"，空串是"清空"。

**两套失败码必须分开**

- 调用方送出的分数/评语/形状被服务自己的接受范围或本模块的形状检查拒绝 ⇒ `assessment_input_*`，SDK 层是 `ErrorCode::InvalidInput`。此时 runtime 把表单**放回** `pending_assessment_form`，所以改正后的答案可以直接重送同一张表单，不必重新读取。
- 请求**已经发出**而服务没有给出 `result == "success"` ⇒ `assessment_submit_unconfirmed`，SDK 层是 `ErrorCode::OutcomeUnconfirmed`。这是一次写动作结果不明，**绝不重放**；只有重新读一次列表才能弄清服务到底存了什么。

这两者不能合并成一个错误，否则"你填错了"和"我不知道发生了什么"在调用方看来会是同一件事。

**一次性：三个层次都拦得住**

1. 适配器在**构造请求之前**就把该行标成 `submitted`（`routes.submitted.push(index)`），随后的传输失败无法与"服务其实已经处理"区分，因此第二次提交同一行返回 `AssessmentAdapterError::SubmitAlreadyAttempted`，直到一次新的列表读取推进 generation。
2. runtime 的 `submit_assessment_form` 不重试，失败即 `invalidate_assessment_session()` 并记录业务失败。
3. FFI 句柄在派发前就被 `remove()`，同一个 `reference_id` 用第二次直接落到未知句柄。

**Runtime 接线**

新增状态字段 `pending_assessment_form: Option<AssessmentEvaluation>`（`direct_state_field_count` 109 → 110）：它是"最近一次从列表行读到的、已经过解析与校验的表单"。`load_assessment_form_result(reference)` 走 `allow_live_operation` → `ensure_identity_user_for_live_read` → 要求 `assessment_service_is_proven()` → `adapter.read_form(reference)`，成功后把求值对象存进 `pending_assessment_form` 并清空失败码；失败时按 `diagnostic_code()` 记录，会话失效或 `StaleForm`/`ForeignForm`/`UnknownForm` 一律作废该会话。`submit_assessment_form(answers)` 先取出持有的表单，reference 不符即拒（不消耗表单），再 `apply_to`，失败时把表单放回；成功或结果不明都清空，避免同一张表单被写第二次。`invalidate_assessment_session()` 与 `logout` 都会清掉它——未保存的内存会话在进程退出后不作任何"仍可复用"的声明。

**SDK / FFI / Dart**

- SDK：`pub mod assessment` 增列 `ASSESSMENT_MIN_SCORE`(1) / `ASSESSMENT_MAX_SCORE`(7)、`AssessmentAnswers`、`AssessmentPersonAnswers`、`AssessmentQuestionAnswer`、`AssessmentFormView`、`AssessmentPersonView`、`AssessmentQuestionView`、`AssessmentPersonRole`、`AssessmentInputError`，`AssessmentClient` 新增 `form(&AssessmentRef)` 与 `submit(&AssessmentAnswers)`。
- FFI：新增 `AssessmentFormDto` / `AssessmentPersonDto` / `AssessmentQuestionDto` / `AssessmentQuestionAnswerDto` / `AssessmentPersonAnswersDto` / `AssessmentAnswersDto`，`AssessmentListItemDto.reference_index: u32` 改为 `reference_id: String`——行引用沿用 usereg 设备与发票文档的既有做法：`ClientHandle` 持 `HashMap<String, AssessmentRef>`，每次列表读取开始前清空、在 `invalidate_auth_bound_references` 里清空，对每行发出一个新的 UUID；引擎的 `AssessmentRef::index` 不出现在公开 surface 上。所有新 DTO 的 `Debug` 手工脱敏（只打印 `reference_present` / 计数 / `has_comment` / `comment_editable`），因为课程名、题干与评语都是账号相关的服务文本。`AssessmentAnswersDto` 同样手工实现 `Debug`，否则 derive 会把 `reference_id` 原样打印出来。FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/assessment.dart` 补齐 `AssessmentForm` / `AssessmentQuestion` / `AssessmentPerson` / `AssessmentQuestionAnswer` / `AssessmentPersonAnswers` / `AssessmentAnswers`，`AssessmentClient` 增 `form({referenceId})` 与 `submit({answers})`，`lib/assessment.dart` 入口同步导出。

**验证**

`cargo test -p tsinghua_kit_engine --lib -- assessment` **43 项通过**（`assessment_tests` 34 项 + `api::runtime::assessment_tests` 9 项）：列表解析与拒绝未知层级、表单解析（事务字段、整体评语、每人每题）、显示副本携带服务当前答案、答案集被应用到它所对应的那张表单、形状不符的答案集被拒、视图与答案的 `Debug` 都不含提交状态、一次性提交被确认/被拒/被不重放、未确认提交不重放、被拒答案集在**发送之前**就返回且表单可复用、行引用跨会话不作废他人、adapter `Debug` 不含路由或账号文本；Runtime 9 项覆盖：列表读取落在既有 jxgl 映射根内、按行引用读表单（断言 GET 落在 `/http/{ASSESSMENT_MAPPING}/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1001&kcbh=1` 且不带 `ticket`）、提交把答案写进服务自己的表单并只派发一次（断言 body 含 `wjid=1001`、`pjfs_1=7`、`jtjy_1=%E8%AE%B2%E5%BE%97%E5%BE%88%E5%A5%BD`，且第二次提交**零请求**）、未确认提交不被重放、对未打开的行提交答案被拒、未证明账号零请求。

SDK `cargo test -p tsinghua_kit --all-targets` 28 项通过（`client_api` 12 + `public_api` 16；其中 `tests/public_api.rs` 新增两项：`assessment_reads_require_identity_without_hiding_the_service` 断言无身份时是 `Service::Assessment` + `SessionRequired`；`assessment_answers_are_bounded_and_redact_their_comments` 断言越界分数被拒、评语原文不出现在 `Debug`），`rust_consumers_can_import_curated_domain_modules_without_ffi` 增补全部新公开类型的导入与 `ASSESSMENT_MIN_SCORE == 1` / `ASSESSMENT_MAX_SCORE == 7` 断言。FFI `cargo test -p tsinghua_kit_ffi --features ffi-bridge --lib` 38 项通过，其中两项为本轮新增：`assessment_writes_reject_unknown_row_handles_before_reading`（伪造 UUID 的表单与提交都在读之前以 `assessment`/`context_mismatch` 被拒，且账号仍是 `SignedOut`）与 `assessment_bridge_debug_omits_questions_comments_and_handles`（两种 `Debug` 渲染都不含课程名/教师名/题干/评语/答案/句柄原文）。

`cargo check --workspace --all-targets` 退出 0（engine lib 的 warning 数与 HEAD 逐条相同，均为 71 条既有无关项，无新增；本轮新增的 `SUGGESTION_INPUT_MARKER` 未使用警告在收尾时删除了该常量，因为题干里的评语输入实际是按 `class` 属性而非按这个字面量识别的，留着它只会误导后来者）；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`cargo fmt --all -- --check` 与 `git diff --check` 干净；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `AssessmentForm` / `AssessmentQuestion` / `AssessmentPerson` / `AssessmentAnswers` / `AssessmentPersonAnswers` / `AssessmentQuestionAnswer` 与 `minScore == 1`、`maxScore == 7`。`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新：`source_revision` 前进、模块列表仍 61、根 `pub use` 计数仍 51、`rendered_crate_root_item_counts` 为 struct 301 / enum 143 / constant 45（新增的正是本轮 9 个 struct、2 个 enum、3 个常量）、DTO 清单新增 6 项、方法清单新增 3 项（含 §57 的 `load_course_score_result`）、`direct_state_field_count` 110。同时修掉三处本轮新引入的 rustdoc 断链（`campus_html`、`AssessmentQuestion`、`parse_assessment_form_html` 与 `apply_to` 指向私有项），并把 §57/§58 遗留的文档一致性问题一并更正：`ThosCourseScoreDto` 定义在 `api/thos.rs` 而不在 `runtime.rs`，baseline 的方法行号此前取自 §55 的源码状态，现已按当前源码重新抽取（行号、签名、顺序全部与 `struct.CampusRuntime` 的 Rustdoc 源码锚点逐一核对一致）。

**边界**

本轮**未执行任何真实账号登录或学校服务请求**；本域的线上可用性仍未验证，需另行真实只读验收（且当前只读验收口径**不覆盖**提交动作：写路径只能靠 fixture 证据）。真实提交表单、评语与分数都不进 DTO、日志、台账与 Git；`thu_reference` 侧的相关实现仅作为路径/字段/选择器/可观察行为的证据使用，未复制其源码、夹具或资源。

## 60. 2026-09-29 宿舍洗衣与清紫源泉订水的第三方只读域

本轮补上 `thu_reference` 已有、TsinghuaKit 完全缺失的两类宿舍生活能力：**宿舍洗衣机状态**（杰力洗衣 / 海乐生活 / 小蓝洗衣三家第三方厂商）与**清紫源泉桶装水订水户查询**。两者都不是学校服务、都没有校园账号绑定、都不在 WebVPN 允许名单内，因此本节的重点是"为什么它们可以进这个仓库"以及"边界画在哪里"，而不是路径。

**为什么不需要新增 selector、映射或`Service` 认证**

这三家洗衣厂商各自运营自己的主机（`api.cleverschool.cn`、`yshz-user.haier-ioc.com`、`wash-ltd-thu.aajax.top`），订水厂商是 `dingshui.bjqzhd.com`（**明文 HTTP**，该部署没有 TLS 监听）。它们既不识别校园账号，也不接受任何校园凭据。参考实现让用户在主页面里自己输入楼栋与订水编号，从不经过 INFO 登录。因此本轮**不新增允许名单条目、不新增 `Service` 变体、不扩展 `map_additional_roaming`**：把它们登记进 INFO 的 roam 允许名单会凭空造出一个并不存在的账号绑定。

**请求姿势：Cookie-free 但仍在统一门禁内**

两个适配器都用 `CampusHttpTransport::with_timeout` 建**自己的**传输层（`washers` 用 `THYou/laundry`，水用 `THYou/water`）：

- 它自带一个全新的 `CampusCookieStore`，所以哪怕复用同一个传输类型，也不可能把校园 Cookie 带到第三方主机上。
- `execute_once` 对非 loopback 目标仍然走 `crate::request_gate::campus_request_gate()`，且 POST 依旧按 `dispatch_requires_exclusivity` 走独占派发。因此"所有实际请求仍经过 Rust `CampusHttpTransport` 的统一节流入口"与"不绕过 transport 直接调 reqwest"两条同时成立。
- 适配器只暴露 `client()` 来**构造**请求，`send` / `execute` 仍是 `pub(crate)`，第三方域拿不到旁路。

**读是账号无关的，所以不套校园登录**

`laundry_api.rs` 的模块文档把这条写成硬规则：读第三方厂商**不需要** `ensure_identity_user_for_live_read()`，也不需要 `service_session_is_proven()`。给一个不存在的绑定加门禁，会在 UI 上变成"没登录就不能看洗衣机"，而那是假的。同理**没有任何缓存与持久化**：设备状态按分钟变化，厂商自己的快照时间（`fetched_at_unix`）原样上报，存一份下来写下去的瞬间就是错的。

**"坏掉的部署"与"空结果"必须分开**

三家厂商的响应形状差异很大（杰力的 `errorCode`、海乐的分页 `nearPosition` / `deviceDetailPage`、小蓝的 `buildings/<id>` 信封），而"字段缺失"既可能是厂商改版也可能是真的没数据。判定规则：**观测到的契约里恒定出现的字段**（海乐 `floorName` / `macUnionCode`、小蓝 `deviceId` / `facilities` / `devices` / `buildings`）一旦缺失就是 `UnexpectedDeployment`（"改版了"），而不是静默跳过；只有厂商自己声明为可选的东西才允许为空。杰力一旦给出 `errorCode` 就是 `BusinessFailure`，绝不当作空列表。海乐按分类（`00` 洗衣机 / `01` 洗鞋机 / `02` 烘干机）分别请求，某一类失败时把它的 key 收进 `failed_categories` 一起返回，于是"读全了"和"读了一半"在调用方看来是两件事。海乐的设备跨两个搜索点返回，按设备码去重后**只保留清华点**（用 `清华` 且非 `中学` 过滤），避免把隔壁中学的洗衣机混进来。

杰力的状态文案走白名单：只有 `待机`/`工作`/`运转` 会离开初始的 `Error`，`剩余` 给 ETA，`更新` 跳过。**无法识别的文案一律报 `Error`**，理由写在注释里：一台停止上报可识别状态的机器，正是学生需要看到的东西。小蓝按 `isOnline` / `fault` / `runState`（7 空闲 / 5 工作 / 1 待机）判定。

**订水：下单端点被结构性排除**

`water_read.rs` 只建模 `/auser/getuser.html`（POST 表单 `name=pw&param=<编号>`）。参考库里真实的**下单**端点 `/buy/subs.html` 会把桶装水真送到门口——本模块**根本没有它的常量、方法和类型**，一个测试专门断言模块里不存在 `subs.html` 或 `/buy/` 字面量。不可达的操作比被守卫的操作更强。编号是调用方自己的输入，在进入 body 之前被约束成短 ASCII 令牌（空、超长、含空格或 `/`、`?`、`<` 一律 `InvalidDeliveryId` 且**零请求**），且从不进入 `Debug`。明文 HTTP 被保留（该部署只有这一个监听），代价被三件事收窄：只发编号、响应不是厂商自己的 JSON 信封就拒绝、整次交换不写入任何持久记录。

**SDK / FFI / Dart**

- 引擎：新增 `washer_read.rs`（适配器 + 三家解析器）、`water_read.rs`、`laundry_api.rs`（公开门面与 serde DTO）。`laundry_api` 的 `LaundryError` / `WaterLookupError` 各自带 `diagnostic_code()`；水品牌的 `WATER_BRANDS` 表把厂商自己的标识映射成中文名，表外的品牌原样返回而不是被抹成空。
- SDK：`pub mod laundry` 与 `pub mod water`，`tsinghua-kit/src/lib.rs` 不新增 `Client` 访问器（这两个读不需要 Client 的会话状态，与 `classify_login_stage` 同类，直接是自由函数）。
- FFI：`ClientHandle` 上新增 `laundry_buildings` / `laundry_rooms` / `water_user` / `water_brand_labels`，四个自由函数 `laundry_providers()` / `laundry_statuses()` / `water_brands()`。所有新 DTO 的 `Debug` 手工脱敏（楼栋只打 `id_present` / `name_present` 与厂商 key、楼栋组只打 `building_count`、机器名/类型/房间只打 `*_present`、房间只打 `machine_count`、一次读只打 `room_count` 与厂商自己的失败分类、订水记录只打 `name_present` 与 `address_len`），因为楼栋名、房间号、机器名和住户姓名都是账号相关或个人信息。选项表（厂商/状态/品牌）本身就是厂商的公开标签，保持 `derive(Debug)`。引擎错误到桥接码的映射集中在 `laundry_vendor_code` / `water_error`：配置与调用方输入 ⇒ `invalid_input`、传输失败 ⇒ `network_unavailable`、非 2xx ⇒ `service_unavailable`、越出原点 ⇒ `redirect_refused`、改版/非 JSON/厂商业务失败 ⇒ `invalid_response`；两条映射各自带上自己的 `service`（`laundry` / `water`），订水的错误不冒充洗衣，因此 App 侧的两个回退文案行不会互相串。FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/laundry.dart` + `lib/laundry.dart`、`lib/src/water.dart` + `lib/water.dart`，`TsinghuaKitClient` 增 `laundry` / `water` 两个 facade（与 `bank` / `invoice` 同构），`LaundryClient.providers()` / `statuses()` 与 `WaterClient.brands()` 是静态的，因为它们不需要 Client 实例。

**验证**

`cargo test --manifest-path rust/Cargo.toml -p tsinghua_kit_engine --lib -- washer_read water_read laundry_api` **33 项通过**（洗衣 18 + 订水 9 + 门面 5）：杰力分组与排序、业务失败不是空列表、状态文案含不可识别项时的降级、杰力设备缺房间 ⇒ 改版、海乐跨搜索点去重与过滤（要清华、不要中学）、海乐分类失败上报、小蓝完整信封（5 台覆盖四种状态加一台无状态码）、小蓝未知楼栋/缺设备列表/缺 organization ⇒ 改版、HTML ⇒ `NotJson`、503 ⇒ `HttpStatus`、原点校验拒绝 ftp/userinfo/query/fragment/非 URL、相对路径不能越出原点、厂商 key 往返、状态 key 稳定、未来与过去的完成时间、诊断码互不相同、`Debug` 脱敏；订水的记录读取与表单字段断言、被拒编号零请求、HTML ⇒ `NotJson`、502 ⇒ `HttpStatus`、原点校验、路径封闭、品牌表、诊断码互不相同、`Debug` 不含编号；门面的厂商/状态表闭合、部分读取保留厂商自己的失败分类、未知厂商零请求、诊断码前缀。

桥接层另有 4 项定向测试：楼栋与楼栋组的 `Debug` 只留存在性与计数（`private-building` / `private-key` / `private-label` 都不出现）、机器与房间的 `Debug` 不出现机器名/类型/房间号、一次读的 `Debug` 保留 `room_count` 与厂商自己的失败分类、订水记录只留 `name_present` 与 `address_len`；`laundry_providers()` / `laundry_statuses()` / `water_brands()` 三张表闭合且标签非空；被拒的厂商 key、楼栋 id 与订水编号都在**没有任何请求**的前提下以 `invalid_input` 返回，且 `auth_status` 仍是 `SignedOut`。SDK crate 的 `public_api` 增补这两个域的公开类型与两条自由函数的编译检查（16 项通过，`client_api` 12 项通过）。`cargo check --manifest-path rust/Cargo.toml --workspace --all-targets` 退出 0（新增的 `LAUNDRY_PROVIDERS` / `water_brand_name` 未使用导入在收尾时修正，engine lib 的 warning 数与 HEAD 逐条相同，均为既有无关项）；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 通过；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `LaundryClient` / `LaundryBuilding` / `LaundryBuildingGroup` / `LaundryMachine` / `LaundryRoom` / `LaundryRoomsReport` / `LaundryProviderOption` / `LaundryStatus` 六个状态与部分读取、`WaterClient` / `WaterUser` / `WaterBrandOption`。`docs/api-surface-baseline.json` 已刷新：模块列表 61 → 64（新增 `laundry_api` / `washer_read` / `water_read`）、`root_public_use` 计数仍 51（引擎根 `pub use` 未变）、`runtime.line_count` 仍 27896（本轮未动 runtime）、`scope` 追加本轮说明。

**未验证**：三个洗衣厂商与订水厂商的线上可用性**均未验证**，需要另行真实只读验收；在此之前不得用 fixture 或空结果冒充线上证据。**未执行任何真实账号登录或第三方服务请求**；未实现宿舍卫生分（§58 的结论不变）。

## 61. 2026-09-29 体育场馆资源与预约记录（只读）

本轮补上 `thu_reference` 已有、TsinghuaKit 完全缺失的**体育场馆**域（`体育场馆`）：场地资源（限额 + 已配置手机号 + 当日的可预约时段表）与本人预约记录（未支付 + 已支付两张表）。选择器 `5539ECF8CD815C7D3F5A8EE0A2D72441`、映射 `a5a70f8834396657761d88e29d51367b6a00`，主机 `50.tsinghua.edu.cn`（参考自己的 `SPORTS_MAKE_ORDER_URL` 里直接写着这个主机名，故主机是**有证据的**；`info_session.rs::map_additional_roaming` 新增的 `SPORTS_WEBVPN_TARGET` 分支只声明"这个选择器属于哪个主机"，本身不授予任何权限）。

**不新增认证、不新增 `ServiceId`**

场馆应用挂在 INFO/WebVPN 的既有漫游握手上，因此 `SportsSessionPrerequisite` 只有 `ExistingInfoWebVpnSession` 一个取值：`ensure_sports_reader_session` 复用 `ensure_info_session` + `info.additional_roaming(...)`，与 `physical_exam_read.rs` / `program_read.rs` 完全同构。`error_sdk.rs` 的 `Service::Sports`（`as_str() == "sports"`）已存在，`protocol.rs::ServiceId` 不新增变体——不存在的第二套登录不会被顺手造出来。整个域没有引入任何 CLI/浏览器/系统资源（与 §60 的第三方域不同）：它的请求一律经 `identity.transport()` 这条统一通道。

**页面把数据放在内联脚本里，所以解析器也必须这样做**

场馆是遗留 JSP 部署，时段表**不在文档文本里**：它由 `resourceArray.push({id:…,time_session:…,field_name:…,overlaySize:…,can_net_book:…})` 构建，由 `addCost(…)` 定价、`markResStatus(…)` / `markStatusColor(…)` 标注，两个限额是 `var limitBookCount = '…'` 与 `var limitBookInit = '…'`。引擎没有 `regex` 依赖，而 `campus_html` 的扫描器按设计只读元素——没有一个元素承载这些值。因此本模块自带一个**有界语句扫描器**：只解析语句的实参列表（引号串、数字、`true`/`false`），永不求值、永不执行、永不跨越 `MAX_SCRIPT_CALLS`（4096 次调用）与 `MAX_SCRIPT_ARG_BYTES`（256 字节/实参）、`MAX_RESOURCES` / `MAX_RECORDS`（各 4096 行）。

**四处有意的收窄（都写在模块文档里，且都有对应测试）**

1. **时段只在页面自己的后续语句重复了它的 id 时才被接受。** 参考实现把 `resourceArray.push` 与紧随其后的 `resourcesm.put('id','hash')` **按位置**配对（跨一个惰性正则），本模块改为**按 id 相等**配对，严格更窄：hash 语句写的是别的 id 时，该时段被**丢弃**，而不是借用邻居的 hash。反过来，一个连 `id` 都读不出来的时段条目是**错误**（`UnrecognizedSlot`），不是"少了一个时段"——坏页面不能伪装成空场馆。
2. **未支付报表相对"语义路标"读，不用裸下标。** `campus_html` 拒绝位置下标，所以本模块自己拥有这个决定并为之付费：先用行内**自己的文案**（`网上支付` / `现场支付`）定位到"支付方式"单元格，参考报告的四列值再按与它的固定距离（`NAME_OFFSET=8` / `FIELD_OFFSET=6` / `TIME_OFFSET=4` / `PRICE_OFFSET=2`）取。路标缺失或**有歧义**（一行里出现两个支付方式单元格）⇒ `UnrecognizedRow`，而不是拿别列的值顶上。动作单元格在路标后两格，`book_id` / `pay_id` 只从它携带的 `payNow` / `unsubscribeOnline` / `unsubscribe` 调用的实参里取，同样不按下标。`book_timestamp` 取动作单元格内 `<span time="…">` 属性。
3. **已支付表没有这种路标**（它的行只有一个硬编码方式，参考读的是连续四格），因此本模块只接受页面自己的载体形状——`style="display:none"` 的 `tr` 包一层嵌套 `tbody`，其首行带四格（`PAID_NAME_CELL=2` / `FIELD_CELL=3` / `TIME_CELL=4` / `PRICE_CELL=5`）——载体形状不符 ⇒ `UnrecognizedRow`，而不是一条半填的记录。
4. **`cost` / `price` 原样保留为 `String`。** 观测到的契约没有为它们确立单位，所以不折算成金额；这与 §55/§56 里"金额一律精确整数分"的域形成对照——那两处有明确的元/分证据，这里没有，就不编。

**写操作结构性排除**

参考库的 `saveGymBook`（下单，需图片验证码）、`unsubscribe`（退订）、`newPay` / `newPayForLater`（支付）、`doUpdateContactInformation`（改手机号）、`Kaptcha.jpg` 以及资金结算 `zjjsfw` 的 `check.do` / `webPay.do` 在本模块**没有任何常量、方法或类型**：不可达的操作比被守卫的操作更强。手机号只读、只解码，且从不进入 `Debug`（`SportsResourcesDto` 只打 `phone_present`）；`res_hash` / `book_id` / `pay_id` 是单用途令牌，`Debug` 同样只打 `has_res_hash` / `has_book_id` / `has_pay_id`。

**失败分类与 SDK 映射**

`SportsAdapterError::diagnostic_code()` 给出 15 个互不相同的码（`sports_config` / `sports_input` / `sports_network` / `sports_http` / `sports_origin` / `sports_path` / `sports_auth_required` / `sports_template` / `sports_body_empty` / `sports_table_missing` / `sports_limit_missing` / `sports_row_unrecognized` / `sports_slot_unrecognized` / `sports_phone` / `sports_too_large`），一次测试断言它们两两不同。runtime 新增 `last_sports_failure_code`（与 `last_graduate_income_failure_code` 同构），`client.rs::sports_failure` **先**读该码再回落到账号状态，因此：被拒的场馆参数 ⇒ `InvalidInput`、越出映射的原点 ⇒ `RedirectRefused`、非 2xx ⇒ `ServiceUnavailable`、响应过大 ⇒ `IncompleteResult`、其余页面形状问题 ⇒ `InvalidResponse`；只有"没有记录码"时才退回 `SessionRequired` / `SessionExpired` / …。

**SDK / FFI / Dart**

- 引擎：新增 `sports_read.rs`（适配器 + 有界语句扫描器 + 4 个公开解析函数）与 `#[cfg(test)] mod sports_tests`；`lib.rs` 导出 `SportsAdapter` / `SportsProfile` / `SportsResources` / `SportsResource` / `SportsReservationRecord` / `PAID_METHOD` / 路径常量等。runtime 新增 `SportsResourcesResultDto` / `SportsRecordsResultDto`、`sports_adapter` / `sports_proof` 两个字段、`ensure_/prepare_/load_*` / `invalidate_sports_session` / `sports_service_is_proven`。
- SDK：`pub mod sports`；`tsinghua-kit/src/client.rs` 新增 `Client::sports()` 与 `SportsClient::{resources, records}`（薄包装，与 `ElectricityClient` 同构），两者都 `Service::Sports` + `ReadSource::Live`（永不走缓存：留一份时段表等于把已被订走的场地显示为空的）。
- FFI：`ClientHandle` 新增 `sports_resources_result(gym_id, item_id, date)` 与 `sports_records_result()`，六个桥接 DTO（`SportsResourceDto` / `SportsResourcesDto` / `SportsResourcesResultDto` / `SportsReservationRecordDto` / `SportsRecordsDataDto` / `SportsRecordsResultDto`），`Debug` 全部手工脱敏。FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/sports.dart`（part 文件）+ `lib/sports.dart` 入口，`TsinghuaKitClient` 增 `sports` facade。`SportsResources.data` 与 `SportsResourcesResultDto` 的列表都是 `List.unmodifiable`，所以调用方无法把场馆没发的时段表当成场馆发的。

**验证**

`cargo test --manifest-path rust/Cargo.toml -p tsinghua_kit_engine sports_tests` **16 项通过**：一次资源读上报限额、手机号与时段（含一个 hash 语句写了别的 id 的时段被丢弃、一个完全没有 hash 语句的时段被丢弃）、一次记录读上报两张表、无法识别的手机号文案是错误而 `do_not` 是"合法地没有"、路标缺失/歧义 ⇒ `UnrecognizedRow`、无表格 ⇒ `MissingTable`、空表格 ⇒ 合法的空列表、已支付载体形状不符 ⇒ `UnrecognizedRow`、不可读的时段条目 ⇒ 错误而非更短的列表、登录页/过期页 ⇒ `is_session_expired()`、被拒的场馆值**零请求**、502 ⇒ `HttpStatus`、越出映射的重定向 ⇒ `UnexpectedOrigin`、base URL 归一化（裸主机与映射根两种）、15 个诊断码两两不同、`Debug` 不出现 hash / 手机号 / `BOOK-1` / `PAY-1`。

SDK `public_api` 增补 `compile_sports_api` 编译检查与两项活动断言（**18 项通过**）：无会话时 `resources` / `records` 都以 `Service::Sports` + `SessionRequired` 返回且账号仍是 `SignedOut`；空场馆号、非数字 item、`2026-02-30`、`2026-9-30` 四种参数都在**没有任何会话工作**的前提下以 `InvalidInput` 返回。桥接层另有 2 项定向测试：时段/记录/资源的 `Debug` 只保留 `has_res_hash` / `has_book_id` / `has_pay_id` / `phone_present` 与计数，被拒参数同样零请求且 `auth_status` 仍是 `SignedOut`。`cargo check --manifest-path rust/Cargo.toml --workspace --all-targets` 退出 0（FFI 侧唯一 warning 是 HEAD 就有的 `physical_exam` / `program` 未使用导入，与本轮无关，逐条核对未新增）；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 退出 0；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `SportsClient` / `SportsResource` / `SportsResources` / `SportsReservationRecord`（含 `List.unmodifiable` 与 `cost` 原样保留）。`docs/api-surface-baseline.json` 已刷新：模块列表 64 → 65（新增 `sports_read`）、引擎根 `pub use` 51 → 52、渲染项计数 `struct 301→312` / `enum 143→148` / `fn 50→55` / `constant 45→51`、`runtime.line_count` 27896 → 28148、`direct_state_field_count` 110 → 112、runtime DTO 68 → 74、runtime 公开方法 92 → 94、`scope` 追加本轮说明。

**未验证**：体育场馆域的线上可用性**未验证**，需要另行真实只读验收；在此之前不得用 fixture 或空结果冒充线上证据。**未执行任何真实账号登录或场馆服务请求，也未发起下单、支付、退订或手机号更新**（这些在该模块里根本不存在）。宿舍卫生分的结论（§58）不变。

## 62. 2026-09-29 成绩的旧绩点口径（局部派生，非服务值）

参考实现在成绩页提供一个「新绩点 / 旧绩点」开关：当调用方选择旧口径时，它会用一张本地表把字母成绩换算成绩点，表外成绩沿用服务自己给的值。本轮把这**一个可分离的口径**补进 `CourseGrade::old_grade_point()`。

**为什么它不违反「不得用本地派生冒充服务值」**

教务服务只发**一个**绩点值。旧口径不是第二个服务值，而是**同一个字母成绩在旧表下的读数**，因此接口按这个性质成形：

- 它是 `CourseGrade` 上的**方法**，与服务自己的 `grade_point()` 并列且命名区分（`old_grade_point()`），Dart 侧同样是并列的 `gradePoint` / `oldGradePoint` 两个字段——调用方无法把两者混为一谈。
- 文档明写这是「locally derived alternate, never a second service value」，并要求展示时必须带「这是哪套口径」的标签（与 `PhysicalExamReport::REFERENCE_TOTAL_LABEL` 的处理同构）。
- 表**只列两套口径不同的九个字母**（`A-` 3.7 / `B+` 3.3 / `B` 3.0 / `B-` 2.7 / `C+` 2.3 / `C` 2.0 / `C-` 1.7 / `D+` 1.3 / `D` 1.0），作为**覆盖表**使用：表外成绩（`A+`、`P` 等）原样沿用服务值，绝不凭空造一个。这与参考实现的 `gradeToOldGPA.get(grade) ?? point` 语义一致。
- 缺值仍是缺值：服务在绩点列什么都没印时，`grade_point` 是 `None`，表外成绩的旧口径也是 `None`——不会把「没有」变成 0。

**接线**

- 引擎：`registrar_api.rs` 新增 `OLD_GRADE_POINT_SCALE` 常量、`CourseGrade::old_grade_point()` 与内部 `old_grade_point()`（对成绩文本 trim 后查表）。行数据本身（`RegistrarCourseGrade`）与解析器**未改动**：服务值照旧按原样解析与缓存，旧口径是读取时的派生视图，因此既有缓存 payload 的 schema 与校验都不受影响。
- FFI：`CourseGradeDto` 新增 `old_grade_point`，`Debug` 增 `old_grade_point_present`（与 `grade_point_present` 一样只打存在性）。FRB 2.13.0 重新生成。
- Dart：`CourseGrade` 新增必填 `oldGradePoint`，文档说明它与 `gradePoint` 的差别与标签要求。

**未纳入本轮**：参考实现的 `getReport(bx=…)` 会在额外一页里筛出必修/限选课程并与主表取交集。该行为的证据只有参考自己的 `table-striped tr` 与第 8 格文案，**没有观察到该端点在当前部署上的响应形状**，也不在既有白名单路径里；在拿到真实响应证据之前不实现，也不猜测它的列位。

**验证**

`cargo test --manifest-path rust/Cargo.toml -p tsinghua_kit_engine --lib -- registrar_api::tests` **3 项通过**：九个字母在服务值故意不同（`Some(3.9)`）时仍被覆盖为旧表值、表外的 `A+` / `P` 与空成绩沿用服务值而不被发明、服务值为 `None` 时表外成绩仍为 `None`（缺失不是零）、成绩文本带首尾空白时仍能命中（` B ` ⇒ 3.0、`\tC+\n` ⇒ 2.3）。`cargo check --workspace --all-targets` 退出 0；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 退出 0；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过。渲染项计数与模块数**未变**（新增的是方法与常量，不改变引擎根的 `pub use` 数量）。**未执行任何真实账号请求**；`getReport` 的 `bx` 过滤仍未实现。

## 63. 2026-09-29 INFO 新闻写操作（收藏与订阅规则，一次性写）

计划阶段 2 的 News 写操作落地：把 `NewsClient` 从"只读"扩到**四个一次性写**——收藏条目增/删（`/b/info/gxfw_fg/common/addFavorite/XXFB/{id}`、`/delFavorite/XXFB/{id}`）与订阅规则增/删（`/b/info/gxfw_fg/common/addSubscribeCondition`、`/deleteSubscribeCondition/{id}/XXFB`）。四条路由、字段名与 `_csrf` 位置都来自参考实现 `thu-info-lib/src/lib/news.ts` 与 `constants/strings.ts` 的**观测值**（两处收藏是带 `_csrf` 查询参数的 GET；订阅新增是 `POST` 表单 `dygz` + `mkid=XXFB`；订阅删除是带 `_csrf` 的 GET，`{id}` 后必须跟 scope 段 `XXFB`）。**未新增 selector、未新增映射、未新增 `ServiceId`**：四条路由都落在既有的 INFO 映射（`f9f94793…`）与既有 `Service::News` 上，因此这个域不需要任何新的认证或允许名单条目。

**调用方不能自己造标识符：引用只由本 Runtime 为同一账号发出**

四个写方法都**不接受调用方自造的 id**：

- 收藏增删收 `ArticleRef`（`generation: u64` + `article_id`）。该引用由新闻列表/搜索读返回，`generation` 是**链接快照代数**：任何一次列表读里把同一篇文章的链接换掉，就会重置整个快照并递增代数，于是旧引用立刻失效（`runtime.info_news_article_reference_is_current`）。runtime 层先解析引用、再建计划，解析失败**在任何请求存在之前**返回 `INFO 新闻条目引用已失效，请重新打开列表`。
- 订阅删除收的是**订阅规则读**发出的 opaque selector，runtime 存的是 `selector → rule_id`，并且额外绑定**账号**与 **300 秒**年龄（`selected_info_subscription_rule` 同时校验 owner 与 `info_subscription_at`）。别的账号的 selector、过期 selector 与未知 selector 一律拒绝，且**零请求**。
- 订阅新增不接受 id，只接受 channel/source 的 **wire 值**（SDK 层已经从本 client 的 `NewsCatalog` 引用解析出来）与可选关键字；`channel` 与 `source` **都为空**时在 Rust 内拒绝（服务无法表达这样的规则），同样零请求。
- 桥接层（FFI）把这件事做到更严：`ClientHandle` 为每个引用类型维护自己的 handle 表，**只认自己发过的 handle**；伪造/别的 client 的 handle ⇒ `context_mismatch`，且**收藏与订阅删除的 handle 在 dispatch 之前就被移除**，所以一次结果不确定的写不能经同一个 Client 再发一次。`news_add_subscription` 需要 `NewsChannelRef` / `NewsSourceRef`（`ArticleRef` 同构，`NewsSubscriptionRef` 则是**单次使用**）。

**三层"只发一次"**

1. **适配器层**：`NewsProfile::{add_favorite_request, remove_favorite_request, add_subscription_request, remove_subscription_request}` 产出 `NewsRequestPlan`；`InfoSessionAdapter::execute_news_write` **拒绝非写计划**（`InvalidConfig`，零请求），只补 `_csrf`、只经统一 transport 发一次。`classify_news_write` 把响应分成 `Accepted`（`result == "success"`）/ `Refused`（业务失败包）/ `LoginRequired` / `Unrecognized`（空体、超限、非 JSON、结构不符）——**只有 `Accepted` 是成功**。
2. **transcript 层**：写请求**不走有界读派发**。两条收藏与订阅删除是**带 `_csrf` 的 GET**，`dispatch_requires_exclusivity` 无法从请求形状识别，因此新增 `CampusHttpTransport::execute_once_exclusive`（`execute_once` / `execute_once_with_exclusivity` 共享实现，`exclusive: Option<bool>`）**显式取整个 gate**（`acquire_for(true)`），并且**只返回第一个响应、不跟随重定向**——一次重定向跳转就是一次"结果已不确定的写"的重放。传输失败（请求已经离手）也归类为 `Unrecognized`，错误文本被**丢弃**：reqwest 的失败信息可能带上请求 URL，而该 URL 里带 `_csrf`。
3. **runtime 层**：订阅新增的**条件指纹**（channel/source/keyword 的 JSON 三元组）在**建计划之前**就记入 `info_news_dispatched_subscriptions`；重复条件在 Rust 内拒绝并记 `info_news_write_replayed`（⇒ SDK `ContextMismatch`），服务端根本看不到第二次。只有**一次新的订阅列表读**才清空该集合——那份列表是服务自己对"现在有哪些规则"的回答。`Refused` / `Unrecognized` 记 `info_news_write_unconfirmed`（⇒ SDK `OutcomeUnconfirmed`），`LoginRequired` 走与其他 INFO 读相同的**一次**有界续接并作废 INFO 会话（`info_session_expired`），**并且仍然不重发**：调用方必须自己再决定一次。

**缓存一致性（收藏写的副作用）**

收藏写会改变每一份列表/搜索缓存里的 `favorited` 标记，而缓存键是账号 + 查询、不含该标记。因此新增 runtime 字段 `info_news_favorite_write_epoch: Option<(账号, 时刻)>`，`Accepted` 的收藏写会写入它；列表与搜索的缓存守卫额外要求 `info_news_page_cache_is_still_current`（**只有同一账号**、且 `generated_at >= written_at` 的页面才算仍然可用）。收藏与订阅读本来就**不走缓存**，因此不查这个记录。登出与 INFO 会话作废时两个字段一并清空。

**七条固定拒绝必须显式进 `public_error` 白名单**

`public_error` 先看逐字白名单（`error == message` 时原样返回），然后才走小写子串 if 链，最后落到 `if normalized.contains("info")` 的兜底。上述前六条拒绝都以 `INFO ` 开头，若不进白名单就会被兜底重写成"INFO 服务暂时不可用，请稍后重试"——**把"去刷新列表"误导成"稍后重试"**。本轮把六条新增拒绝 + 一条既有拒绝（`INFO 新闻编号无效`）都加入白名单。

**一处必须固定文本的拒绝**

`build_plan` 把 `NewsProfileError` **一律**映射为固定文案 `INFO 新闻写入参数无效，请刷新后重试`，而不是透传错误自身的文本。两个原因：(1) `NewsProfileError::InvalidPath` / `InvalidWireName` 携带**违规值本身**，而这里违规值就是服务端标识符，调用方本来不该看到；(2) `public_error` 按子串匹配，未经审查的文本可能被改写成**另一个类别**。参数拒绝发生在任何请求之前。

**SDK / FFI / Dart**

- 引擎：`info_news.rs` 新增四个 `*_request` 构建器 + `NewsSubscriptionDraft`（builder）+ `NewsOperation`（含 `is_write()`）/ `NewsParameterPlacement` / `NewsWriteOutcome` / `MAX_NEWS_WRITE_RESPONSE_BYTES` / `classify_news_write`；新增 `info_news_write_tests.rs`（适配器）与 `api/runtime_news_write.rs` + `runtime_news_write_tests.rs`（runtime）。runtime 新增四个公开方法（`add_/remove_info_news_favorite`、`add_/remove_info_news_subscription`）与两个状态字段。
- SDK：`NewsClient` 新增 `add_favorite` / `remove_favorite` / `add_subscription` / `remove_subscription`（薄包装，参数是引用而非 id）。
- FFI：`ClientHandle` 新增 `news_add_favorite` / `news_remove_favorite` / `news_add_subscription` / `news_remove_subscription`；FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/news.dart` 的 `NewsClient` 新增同名四个方法，`addFavorite` 的文档说明"一次 dispatch，`outcome_unconfirmed` 时引用已被消费，必须重新读 `favorites` 才知道账号现在持有什么"。

**验证**

引擎定向（loopback fixture，无任何真实账号 / Cookie / CSRF 值）：`backend_repair_news` **48 项通过**，其中适配器层 11 项——`addFavorite` 恰好 2 次请求（bootstrap + 写）且断言线上形状含 `/common/addFavorite/XXFB/article-1`、`_csrf=fixture-csrf`；`delFavorite` 用删除路由；订阅新增是**一个**表单且带 `dygz=%7B…` 与 `mkid=XXFB`；订阅删除路径含 `/common/deleteSubscribeCondition/rule-9/XXFB`；服务端拒绝（5 种返回体）**从不重放**；302 **不被跟随**（第三个 fixture 故意未被消费）；登录页 ⇒ `LoginRequired`；500 ⇒ `Unrecognized`；**读计划 ⇒ `InvalidConfig` 且零请求**；无条件订阅 ⇒ `EmptySubscriptionCondition`；路径段穿越矩阵（`../secret`、`a/b`、`a\b`、`a?b`、`a#b`、`a&b=c`、`a%b`）全部拒绝。runtime 层 `backend_refactor_news` **20 项通过**（含 8 项新 fixture + 1 项 `info_news_page_cache_is_still_current` 单元测试 + 1 项条件指纹单元测试）：单次 dispatch 且记录写时刻、被替换的链接使代数失效且旧引用零请求、缺 INFO 适配器时明确断言而不是偷偷开第二条认证链、结果不确定的写**不被重试**、重复订阅条件在 Rust 内被拒（服务端只看到 2 次请求）、空条件零请求、刚读出的 selector 用完即退休（再用即拒）、别的账号的 selector 被拒。SDK `public_api` **19 项通过**（新增 `ArticleRef` 导入、`compile_news_api` 里四个写调用的编译检查、`accepts_public_types::<ArticleRef>`、以及"无会话时每个写都以 `Service::News` + `SessionRequired` 返回且账号仍是 `SignedOut`"的身份门禁测试）。桥接层 `--lib` **45 项通过**（新增 `news_writes_only_accept_handles_this_client_returned`：伪造/别的 handle ⇒ `context_mismatch`，无条件订阅 ⇒ `invalid_input`，且 `auth_status` 仍是 `SignedOut`）。`cargo check --workspace --all-targets` 退出 0；`cargo fmt --all --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 退出 0（引擎 crate 仍有两处**HEAD 既有**的私有文档链接告警：`assessment_read.rs` → `campus_html`、`invoice_read.rs` → `follow_invoice_handoff`，与本轮无关，逐字比对未改动）；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖四个引用类型（私有构造 ⇒ 调用方只能从本 Client 的读拿到）。

同一 `news` 过滤器下另有 **5 项 HEAD 既有失败**（`backend_repair_business_news_ui_id_uses_the_returned_article_link`、`backend_repair_info_news_cache_miss_lazily_establishes_info`、`backend_repair_info_news_stale_cache_attempts_lazy_refresh_then_falls_back`、`info_session::tests::fixture_promotes_after_cookie_handoff_probe_and_executes_news`、`info_session::tests::news_read_rejects_a_valid_envelope_from_another_target_route`）：用 `git stash push -u` 在**同一 fixture 环境**下重建并重跑，失败集合逐项相同，逐字确认与本轮无关。

**未验证**：四条新闻写路由的线上可用性**未验证**，需要另行真实只读验收——写路由本轮**未对任何真实账号发起**（§59 的写操作约定不变：写操作只做本地验收，真实账号只读验收另行进行）；在此之前不得用 fixture 或空结果冒充线上证据。

## 64. 2026-09-29 图书馆座位预约、预约记录与取消（一次性写）

计划阶段 3 的第一个子域：图书馆**座位预约 / 预约记录 / 取消预约**。三条路由分别来自参考实现 `thu-info-lib/src/lib/library.ts` 的观测值：预约 `POST /api.php/spaces/<seatId>/book`（表单 `segment` / `type` / `operateChannel=2`），预约记录 `GET /user/index/book`（16 列表格），取消 `POST /api.php/profile/books/<id>`（表单 `_method=delete` / `id` / `operateChannel=2`）。**未新增 selector、未新增映射、未新增 `ServiceId`**：三条路由都落在既有的 seat.lib 映射（`e3f24088…`）与既有 `Service::Library` 上，因此这个子域不需要任何新的认证或允许名单条目，也不存在第二套请求路径。

**唯一的预约凭据来自本 Runtime 自己的读**

预约表单里的 `access_token` 是服务自己放在 `/home/web/f_second` 页面里的一次性令牌。它**只**在 `LibraryWriteAdapter::execute_write` 内、在那一次请求之前被读出，随后：

- 不进 `LibraryWritePlan`（`form_parameters()` 里没有它，`Debug` 只打印字段名）；
- 不缓存在 adapter 上，也不跨调用存活；
- 不跨 FFI 边界（`LibraryAccessToken` 的 `Debug` 打印 `[redacted]`，只有 `expose()` 在 Rust 内可取用）；
- 读不到或读不成时**拒绝发请求**（`MissingAccessToken` / `InvalidAccessToken` ⇒ SDK `unsupported`），而不是发一个空令牌。

**座位类目由 Rust 自己保管，不是调用方参数**

预约表单的 `type` 是座位可预约数据里的 `area_type`。它来自本 Runtime 刚读到的座位清单，保存在**按 `(section_id, segment_id)` 双键**的运行时状态 `library_confirmed_seats` 里（每条记录只含 `is_available` 与 `area_type` 两个值）。双键是刻意的：客户端 `seats()` 会同时推进窗口引用，若只按 section 存，客户端就可能拿一个**它从未读过的窗口**去预约。

于是 `book_library_seat(section_id, segment_id, seat_id)` 的三项证据必须**同源**：

1. section ∈ 当前已验证目录发出的 section 集合（`library_seat_hierarchy_active` 打开时）；
2. segment ∈ 本 Runtime 为该 section 确认过的开放时段；
3. seat ∈ 本 Runtime 为**同一** (section, segment) 读到的座位清单，且 `is_available` 为真。

任一不成立即在**任何请求存在之前**以 `library_section_unconfirmed` / `library_segment_unconfirmed` / `library_seat_unconfirmed` / `library_seat_unavailable` 拒绝（分别映射到 SDK 的 `ContextMismatch` / `NotAvailable`）。`area_type` 因此不是 FFI、SDK 或 runtime API 的参数——`LibrarySeat` 也已随之不再携带该字段。

**取消只能来自本 Runtime 自己的预约记录读**

`load_library_reservations` 是**唯一**能产生取消选择器的操作。它读取预约记录后，为每一行**仍带服务自己的取消控件**的记录铸一个 `Uuid` 选择器，runtime 存 `selector → cancellation_id`，并**同时**记录 owner 与 `Instant::now()`（复用既有 `selected_info_subscription_rule` 的 owner + 300 秒双重校验）。于是别的账号的选择器、过期的选择器与未知选择器一律拒绝且**零请求**，服务侧的取消标识符（`menuDel(...)` 的参数）从不离开 Rust。不含取消控件的行不铸选择器——那是**服务自己**对该行的表态，不是读取失败。

**"只发一次" 的三层落点**

1. **适配器层**：`execute_write` 只接受写计划（`WriteOperation`，零请求），只经统一 transport 发一次，且必须经 `execute_once_exclusive` **显式取整个 gate、不跟随重定向**（一次重定向就是一次结果已不确定的写的重放）。传输失败被归类为 `Unrecognized`，**错误文本被丢弃**（reqwest 的失败信息可能带上带令牌的请求 URL）。
2. **分类层**：只有 `Accepted` 是成功。`Refused`（业务失败包）与 `Unrecognized`（空体、超限、非 JSON/HTML、结构不符、跨源或越界重定向、非 2xx）都记 `library_write_unconfirmed` ⇒ SDK `OutcomeUnconfirmed`；`LoginRequired`（WebVPN 登录页 / HTTP 200 的会话过期页 / 401 / 403）作废 LIBRARY 会话记 `library_session_expired` ⇒ SDK `SessionExpired`。**两者都不重放**。
3. **桥接层**：`ClientHandle` 为座位与预约各维护自己的 handle 表，**只认自己发过的 handle**（伪造 ⇒ `context_mismatch`）；`library_reserve` 与 `library_cancel` 都在 **dispatch 之前**把 handle 从表里移除，所以一次结果不确定的写不能经同一个 Client 再发一次。一次新的座位读或预约记录读才会产出新的 handle。

**必须固定的两条拒绝文案**

`record_business_failure("library", …)` 会把失败码写进 `last_library_failure_code`，`library_failure` 先查该字段再谈别的，映射表把 `library_write_unconfirmed` 归到 `OutcomeUnconfirmed`（**不可重试**）、把 `library_session_expired` / `library_auth_required` 归到 `SessionExpired`、把 `library_account_changed` 与三类 `*_unconfirmed` 归到 `ContextMismatch`、把 `library_seat_unavailable` 归到 `NotAvailable`、把 `library_booking_token` 归到 `Unsupported`、把 `library_write_request` 归到 `InvalidInput`。

**SDK / FFI / Dart**

- 引擎：新增 `library_write.rs`（路径常量 + `LibraryWriteProfile` + `LibraryWritePlan` + `LibraryAccessToken` + `classify_library_write` + `parse_booking_records` + `LibraryWriteAdapter`）与 `api/runtime_library_write.rs`（`ensure_library_write_session` / `finish_write` / 三个入口）；新增 `library_write_tests.rs`（22 项。其中 `read_booking_records` 与预约/取消各覆盖正常、空表、会话过期、拒答、不可读答复五个分支，另有"非学生账号在**零请求**前被拒"与"预约计划不能走取消入口"两条）。runtime 新增三个公开方法（`load_library_reservations` / `book_library_seat` / `cancel_library_booking`）、`LibraryReservationsDto` / `LibraryReservationDto` / `ConfirmedLibrarySeat` 三个类型与五个状态字段（`library_confirmed_seats`、`last_library_failure_code`、`library_reservation_selectors` / `_owner` / `_at`）。`LibraryReservationRef` 改为 opaque 选择器 + owner + 客户端代数；`LibraryBookingRecord` 的 `Debug` 改为只打印 `cancellable: bool`；`LibrarySeat` 去掉 `area_type`，`LibraryBookingRequest` / `bind_booking` 删除。
- SDK：`LibraryClient` 新增 `reservations` / `reserve(&SeatWindowRef, &SeatRef)` / `cancel(&LibraryReservationRef)`，`library` 模块 re-export 三个新类型。
- FFI：`ClientHandle` 新增 `library_reservations` / `library_reserve` / `library_cancel`；`LibraryReservationDto` 的 `Debug` 只留 `cancellable`；FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/library.dart` 新增 `LibraryReservationReference` / `LibraryReservation` / `LibraryReservations` 与三个同名方法，`lib/library.dart` 导出三个新类型。

**验证**

引擎定向（loopback fixture，无任何真实账号 / Cookie / 令牌值）：`library_write` **22 项通过**——预约恰好 2 次请求（令牌读 + 写）且断言线上形状含 `POST /api.php/spaces/701/book`、`access_token=fa-9b7c`、`userid=<fixture>`、`segment=9001`、`type=2`、`operateChannel=2`，而 `LibraryWritePlan` 的 `Debug` 既不出现令牌也不出现账号；取消恰好 2 次请求且断言 `POST /api.php/profile/books/202009111837`、`_method=delete`、`id=202009111837`；拒答与不可读答复各**只发一次**；缺令牌时零写请求且错误文本不含 `access_token`；16 列布局被换掉时以 `InvalidRecord` 拒绝而不是把相邻列当成预约；`Debug` 断言预约记录不泄露 `menuDel` 的参数但保留 `cancellable: true`。runtime 层 `a_selector_from_an_older_read_is_not_accepted` 通过（别的账号 / 未知 / 301 秒三种失效）。桥接层新增 2 项（`library_writes_only_accept_handles_this_client_returned` 与 `library_reservation_bridge_debug_omits_the_cancellation_handle`），`cargo test -p tsinghua_kit_ffi --lib --features ffi-bridge` **47 项通过**。SDK `public_api` **20 项通过**（新增 `compile_library_api` 里的预约/记录/取消编译检查与 `library_reservation_entries_report_the_missing_session_instead_of_an_outcome`），`client_api` 12 项通过。`cargo check --workspace --all-targets` 退出 0；`cargo fmt --all -- --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 退出 0；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖三个新类型与"服务未给取消控件时引用为 null"这一语义。THYou 侧新增 `('library', …)` 七条文案（其中 `outcome_unconfirmed` 明确读作"请勿重复提交、可刷新预约记录查看"），`flutter test test/tsinghua_kit_failure_test.dart` 10 项通过。

同一 `library` 过滤器下另有 **4 项 HEAD 既有失败**（`backend_repair_cached_library_read_uses_existing_expiry_gate`、`backend_repair_proven_library_action_survives_unavailable_info_bootstrap`、`backend_repair_runtime_library_segments_and_seats_reach_safe_dtos`、`backend_repair_binding_library_timestamp_date_must_match_segment_day`）：用 `git stash push -u` 在**同一 fixture 环境**下重建并重跑，失败集合逐项相同（HEAD 58 通过 / 4 失败，本轮 80 通过 / 4 失败），逐字确认与本轮无关——它们直接安装 library 证明却从不填充 `library_section_ids`，因此 `load_library_day_segments` 会以 `library_section_unconfirmed` 拒绝该 section。本轮**未修**这四项：它们与本次新增的写路径无关，改动它们会掩盖真正的原因。

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新：模块列表 65 → 66（新增 `library_write`）、引擎根 `pub use` 52 → 53、渲染项计数 `struct 313→320` / `enum 151→155` / `fn 55→58` / `constant 51→55`（`trait` 与 `type` 不变）、`runtime.line_count` 28278 → 28436、`direct_state_field_count` 115 → 120、runtime DTO 74 → 76、runtime 公开方法 98 → 101。本次刷新同时更正两处**本次之前就存在**的记录错误：上一版的 `runtime.line_count` 记成 28279，而该文件在上一版修订上实测 28278（本文件因此看起来移动了 157 行，实际是 158 行）；上一版 98 条方法的行号取自更早的源码状态，本次按当前源码**逐条重新抽取**（98 条全部右移 63 行），因此行号与签名现在与 `struct.CampusRuntime` 的当前源码一致。

**未验证**：三条路由的线上可用性**未验证**，需要另行真实只读验收——预约与取消本轮**未对任何真实账号发起**（§59 的写操作约定不变）；预约记录这一条读也尚未在真实账号上执行过。在此之前不得用 fixture 或空结果冒充线上证据。

## 65. 2026-09-30 图书馆插座开关（一次性写）

计划阶段 3 的第二个子域：图书馆**插座开关**（把某座位的电源插座打开或关闭）。这条写与前一个子域（座位预约）**不共用同一套请求路径**：它落在独立托管的校园 App 源 `app.cs.tsinghua.edu.cn`，走 `POST /api/socket`，请求体是 JSON `{"seatId": <u64>, "isavailable": <bool>}`，**不带**图书馆预约令牌、也**不带**账号 id。这一段的只读侧（`GET /api/socket?sectionid=<id>`）引擎早已实现（`library_read.rs` 的 `LibrarySocketStatusAdapter`），本子域补的是它的写方向。

**未新增 selector、未新增映射、未新增 `ServiceId`**：源与路径都在既有的 socket 常量里（`LIBRARY_SOCKET_STATUS_ORIGIN` / `_HOST` / `_PATH`），服务仍是既有的 `Service::Library`。因此这个子域不需要任何新的认证或允许名单条目。

**为什么它不是 `LibraryWriteAdapter` 的第三个操作**

预约与取消都把座位库存映射当成自己的上下文，并且都在表单里带 `access_token` 与 `userid`。插座路由两样都不是。若把 `SetSocketState` 塞进 `LibraryWriteOperation` 与 `LibraryWritePlan`，就会存在一条能把**带到预约令牌的表单**发到**另一个源**的路径。因此本轮新增的是**独立的计划类型**（`LibrarySocketWritePlan`）与**独立的适配器**（`LibrarySocketWriteAdapter`）：

- `LibrarySocketWritePlan` 的字段是 `operation` / `method` / `path` / 私有 `seat_id` / 私有 `is_available`；`body_fields()` 只回 `["seatId", "isavailable"]`，`body()` 只插入这两个字段，`Debug` 只打印 `operation` / `method` / `has_relative_path` / `body_fields`——**座位号不进 `Debug`**。
- 因为两个计划类型彼此不可互换，`LibraryWriteAdapter` 无法被喂进一个 socket 计划、反之亦然，这一点由类型系统而不是运行期检查保证；跑不出来的那个方向无从测试，可编译的那个方向由 `socket_state_request(0, _)` 的 `InvalidIdentifier` 覆盖。
- 适配器复用 `library_read.rs` 里那两个原本私有的 socket 源校验函数（`validate_socket_base_url` / `normalize_socket_base_url`，本子域把它们升为 `pub(crate)`），而不是复制一遍源/路径校验。`LibraryAdapter::app_socket_write_adapter()` 是它唯一的正式构造入口，刻意与 `write_adapter()` 分开。

**验收规则是本模块自己的判定，不是从参考实现继承来的**

参考实现对这条路由**没有任何验收证据**可抄：`thu-info-lib/src/lib/library.ts` 的 `toggleSocketState` 发完 `uFetch(APP_SOCKET_STATUS_URL, …, "application/json")` 就 `.then(() => {})`，而 `uFetch` 只在非 200/201 时抛错；`f856fe5d` 之前的 `webApi.ts` 版本形状相同（`if (!resp.ok) throw`）。在整个 `thu_reference`（16 个仓库 + 文档 + mock）里检索 `api/socket|isavailable|toggleSocket` 只命中这三处 `thu-info-lib`，且 App 侧**没有 `toggleSocketState` 的调用点**。所以"HTTP 200 就算成功"并不是观测到的语义，只是参考库的省略。

本模块因此**显式写下**自己的判定（`classify_socket_write`），并把这一事实写进测试名 `backend_repair_library_socket_write_acceptance_rule_is_stated_not_inherited`：

- **接受**：空体 / 纯空白体 / 大小写不敏感的 `OK` / JSON 外层里 `status` 或 `result` 或 `success` 为整数 `1` / 布尔 `true` / 字符串 `"success"`（且无失败标记）。空体被接受是唯一从部署行为推断而非从标志位读出的形状，注释里写明了这一点。
- **拒绝**（`Refused`）：JSON 里的 `success == false`、`result == false`、或其他整数的 `status`/`result`/`success`、或非 `"success"` 的字符串，或 `message`/`msg`/`error`/`errorMessage` 里带失败措辞。数值 `status: 0` **不**单独算失败标记（与只读侧的判断保持一致），它作为整数走上面的规则。
- **不可读**（`Unrecognized`）：HTML、超过 `MAX_WRITE_RESPONSE_BYTES`、未知 JSON 形状、非整数数字。`Unrecognized` **不重放**。

**座位证据来自本 Runtime 自己的读，但座位句柄不花掉**

`set_socket_state(section_id, seat_id, is_available)` 的两项证据与预约同源：

1. section ∈ 当前已验证目录发出的 section 集合（`library_seat_hierarchy_active` 打开时），否则 `library_section_unconfirmed`；
2. `(section_id, _)` 下本 Runtime 确认过的座位清单里含该 `seat_id`，否则 `library_seat_unconfirmed`。

**刻意不查** `area_type` 与 `is_available`——插座服务有它自己的状态，这里要的只是座位的出处。会话证据用的是 `ensure_library_reader_session` 而**不是** `ensure_library_write_session`：这条请求不带预约令牌也不带账号 id，父 INFO 写证明在这里既不必要也不成立；"只发一次"仍由 `execute_once_exclusive`（取整个 gate、不跟随重定向）保证。

插座**不能**像座位那样在派发前花掉句柄：`library_sockets` 正是以那个座位句柄为键的，花掉它就会让"结果不确定时重新读插座"这条恢复路径变得不可达。所以桥接层与 SDK 都保留该句柄，只靠适配器与 runtime 保证单次派发。

**SDK / FFI / Dart**

- 引擎：`library_write.rs` 新增 `SetSocketState`、`LibrarySocketWriteProfile` / `LibrarySocketWritePlan` / `classify_socket_write` / `has_socket_write_failure_marker` / `LibrarySocketWriteAdapter`；`library_read.rs` 新增 `LibraryAdapter::app_socket_write_adapter()` 并把两个 socket 源校验函数升为 `pub(crate)`；`api/runtime_library_write.rs` 新增 `set_socket_state`，`CampusRuntime` 新增一个委托方法；`library_write_tests.rs` 新增 7 项。
- SDK：`LibraryClient` 新增 `set_socket_state(&LibraryAvailability, &SeatRef, bool)`（薄包装，委托引擎客户端）。
- FFI：`ClientHandle` 新增 `library_set_socket_state`；`library_writes_only_accept_handles_this_client_returned` 增补两条插座用例（伪造的一对、伪造窗口 + 空座位，都断言 `service == "library"` 且 `code == "context_mismatch"`）；FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/library.dart` 新增 `setSocketState(availability, seat, available: …)`——它不引入任何新公开类型，用的就是既有的 `LibraryAvailabilityReference` / `LibrarySeatReference`。

**验证**

引擎定向（loopback fixture，无任何真实账号 / Cookie / 座位号）：`library_write` **29 项通过**（前一个子域的 22 项 + 本子域 7 项）。插座 7 项断言：恰好 1 次请求且是 `POST /api/socket`；`Content-Type: application/json`；体内恰好出现 `"seatId":701` 与 `"isavailable":true`；体内**不出现** `access_token` 与 `userid`；`LibrarySocketWritePlan` 的 `Debug` 不出现 `701`；关方向序列化为 `"isavailable":false`；空体被接受而失败措辞被归类为 `Refused`；HTML 与未知 JSON 形状都是 `Unrecognized` 且**只发一次**；会话过期页归 `LoginRequired`；`seat_id == 0` 在**零请求**前被拒。判定表本身另有一项纯函数用例逐项固定上述四种接受形状与六种不接受形状。

桥接层 `cargo test -p tsinghua_kit_ffi --lib --features ffi-bridge` **47 项通过**；SDK `public_api` **20 项通过**（`compile_library_api` 增补插座调用与"写完之后插座读仍可用"的编译检查）；`cargo check -p tsinghua_kit_ffi --all-targets --features ffi-bridge` 无新增警告（顺带清掉了 `sdk_api.rs` 里**本轮之前就存在**的 6 个未使用导入警告）；`cargo fmt --all -- --check` 干净；`git diff --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 退出 0；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过。

同 `library` 过滤器下的 **4 项 HEAD 既有失败**（`backend_repair_cached_library_read_uses_existing_expiry_gate`、`backend_repair_proven_library_action_survives_unavailable_info_bootstrap`、`backend_repair_runtime_library_segments_and_seats_reach_safe_dtos`、`backend_repair_binding_library_timestamp_date_must_match_segment_day`）与 §64 记录的是同一批：它们直接安装 library 证明却从不填充 `library_seat_section_ids`，因此 `load_library_day_segments` 以 `library_section_unconfirmed` 拒绝该 section。本轮**未修**，原因同 §64（与新增写路径无关）。本子域在 `library` 过滤器下新增 7 项、全绿，因此这一轮该过滤器共 91 项、87 通过 / 4 失败。

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新到第七版：模块列表 66 项**不变**、根 `pub use` 53 条**不变**、runtime DTO 76 个**不变**、`direct_state_field_count` 120 **不变**；移动的是渲染项计数 `struct 320→323` / `fn 58→59`（`enum`/`constant`/`trait`/`type` 不变）、runtime 公开方法 101 → 102（新增 `set_library_socket_state`，插入在 `cancel_library_booking` 与 `load_info_news` 之间，其后 52 条方法统一右移 20 行——与 `git diff --numstat` 的 `20 0` 一致）、`runtime.line_count` 28436 → 28455。本次刷新同时更正两处**本节之前就存在**的记录错误：`runtime.public_free_functions` 的 6 条行号自 `37d4182` 起四轮未随源码更新（`[376, 400, 3127, 3179, 3187, 3218]` → `[390, 414, 3238, 3290, 3298, 3329]`）；上一版的 `line_count` 记成 28436，而该修订实测 28435（本版按 `wc -l` 口径记 28455）。

**未验证**：这条路由的线上可用性**未验证**，需要另行真实只读验收——插座写本轮**未对任何真实账号发起**（§59 的写操作约定不变）；在此之前不得用 fixture 或空结果冒充线上证据。
\n
## 66. 2026-09-30 馆藏教参检索与书目详情（只读）

计划阶段 3 的第三个子域：**馆藏教参**（`教参平台`）的检索与书目详情。它落在自己的校园主机上，经一个固定的 WebVPN 映射进入，走的是 transport 已经持有的 INFO/WebVPN 会话。本子域**只读**，且**没有新增任何 `Service` 认证方式**。

**允许名单条目，以及为什么这一条是"有据"的**

映射令牌是 `77726476706e69737468656265737421` + AES-128-CFB(密钥与 IV 同为 `wrdvpnisthebest!`，明文为主机名)，那个 16 字节前缀本身就是 `wrdvpnisthebest!` 的 ASCII 十六进制。把本模块的令牌 `e2f2529935266d43300480aed641303c455d43259619a3eaf6eebb99` 解码，得到的正是 `reserves.lib.tsinghua.edu.cn`——因此 `map_additional_roaming` 里的新分支把主机写作**有据**而不是推断（与 §60 之后银行 `yhdf`、研究生 `zzjl.graduate` 两条标注为"推断"的分支不同）。方案是 `http`，与参考库自己把这条路由拼成 `/http/<token>/…` 一致。

**刻意不实现参考的恢复策略**

参考库给这个应用登记的策略是 `"id"`，也就是一次**校园身份登录**（先取 `ID_BASE_URL + "5bf6e5a699d63ff1cdb082836ebd50f9"` 的表单，再把凭据 POST 到 `ID_LOGIN_URL`）后重试。引擎不实现第二套校园登录，这里也不需要：Cookie jar 与 WebVPN 跳转已经由 `CampusHttpTransport` 共享。因此 `RESERVES_WEBVPN_TARGET` 只作为**文档常量**保留，说明"这条恢复路径被刻意没有实现"，它**没有**被登记为漫游 selector——这一点由测试 `the_reference_recovery_payload_is_never_registered_as_a_roam_selector` 固定。会话过期统一报 `ReservesAdapterError::SessionExpired`，由既有的 INFO 刷新路径处理，与其它 INFO 承载的读取完全一致。

**没有 mock，也不把"读不出来"当"空目录"**

参考实现在 `.p-fbox` 缺席时返回内置的 `MOCK_RESERVES_LIB_SEARCH = {bookCount: 0, pageCount: 0, data: []}`，这**无法区分**"服务确实没匹配到"与"页面变了"。按 AGENTS.md，本模块改成：**服务自己的结果计数器就是证据**——只有当页面真的印了 `共 0 条结果,0 页` 时才产生空目录；缺计数器、缺某个必填字段、缺书名或缺 `bookId` 一律是解析失败。`请您登录个人INFO账户查看教参全文` 是**明示的登录失败**（`LoginPage`/`SessionExpired`），绝不退化成空结果。

**`%uXXXX`：私有转义必须逐字节活着穿过 URL 层**

参考用 `bookName.charCodeAt(i)`（**UTF-16 码元**），只在 `>= 128` 时输出 `%u` + 大写十六进制。`Url::set_query` 会把 `%` 重新编码成 `%25`，因此查询串是**原样**写入的（`set_query(Some(query))`），再由本模块自己的 `valid_query` 放行——它只接受 `%uXXXX` 与合法 `%XX`。loopback 断言把这个形状钉死在请求行上：`?bookName=%u9AD8%u7B49%u6570%u5B66`，并附注"任何重新编码都会发出 `%25u9AD8…`"。非 BMP 字符因此变成两个代理转义（`"\u{1D11E}"` → `%uD834%uDD1E`）。

**引用不可伪造**

`bookId` 从不离开模块：对外的句柄是 `ReservesRef { adapter_binding, generation, index }`，它绑定到产生它的那一个适配器实例与那一次检索（适配器绑定号取自一个进程内 `AtomicU64`，检索成功后才推进 generation）。换一个客户端、或换一次检索拿到的引用**根本解析不了**，不会读到另一本书。`ReservesRef` 的 `Debug` 只打印 `index`。

**图片与章节链接的重写**

页面印出的封面图与章节 href 一律被改写回本模块自己的映射源（`RESERVES_MAPPING_ORIGIN`）：相对的 `/…` 与已经带该源的绝对地址都接受，`//host`、含 `:`、反斜杠、`#`、`..`、非法百分号编码一律拒绝。这样一份响应无法把调用方的图片或章节抓取搬到别的主机。

**SDK / FFI / Dart**

- 引擎：新增模块 `reserves_read`（14 struct / 5 enum / 3 fn / 5 const）、`ReservesSearch` 与 `ReservesAdapter` 等；`info_session.rs` 新增一条允许名单分支；`api/runtime.rs` 新增 `load_reserves_search_result` / `load_reserves_detail_result` 两个公开方法与三个状态字段；`client.rs` 新增 `ReservesClient`。检索在**任何会话工作之前**先约束书名与页码，被拒的参数不会花掉一次 handoff；检索与会话过期走既有的"失效 → INFO 刷新 → 重试一次"模式，第二次仍过期就失败。详情读**刻意不透明重试**：重建适配器会丢掉书目标识，所以过期时直接失败并提示重新检索，而不是悄悄读另一本书。
- SDK：`tsinghua-kit/src/lib.rs` 新增 `pub mod reserves { … }` 再导出块，`client.rs` 新增 `Client::reserves()` 与薄包装 `ReservesClient`（含 `MAX_PAGE`）。
- FFI：`ClientHandle` 新增 `reserves_search_result` / `reserves_detail_result` 与 `reserves_references` 句柄表（连同 `invalidate_auth_bound_references` 一起清空）；新增 `ReservesBookDto`（`Debug` 只印 `reference_present` 与书名等书目字段，**不印句柄**）、`ReservesSearchDataDto` / `ReservesSearchResultDto` / `ReservesChapterDto` / `ReservesDetailDataDto` / `ReservesDetailResultDto`；FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/reserves.dart`（part）+ `lib/reserves.dart` 入口 + `lib/tsinghua_kit.dart` 的 `part`/字段；`ReservesClient.search({bookName, page})` 与 `detail({referenceId})`，`maxPage = 1000`。总数与页数是 `BigInt`（FFI 的 `u64`）。

**验证**

引擎定向（loopback fixture，无任何真实账号 / Cookie / 书目标识）：`cargo test -p tsinghua_kit_engine reserves_tests` **32 项通过 / 0 失败**，覆盖正常页、空页（计数器为 0）、缺计数器、计数器非 0 而块为空、缺书名 / 缺 `bookId` / 缺字段、非 HTML 响应、5xx、空体、超时文案、WebVPN 门户页、服务自己的未登录提示、图片与章节链接重写与拒绝、私有转义的逐字节形状、引用跨适配器 / 跨检索失效、以及"参考的恢复载荷没有登记成漫游 selector"。

桥接层 `cargo test -p tsinghua_kit_ffi --lib`（含 `--features ffi-bridge` 与不带该特性两种配置）**49 项通过**，其中本轮新增 2 项（`reserves_bridge_debug_omits_the_opaque_row_handle`、`reserves_refusals_need_no_account_and_no_request`）。SDK `cargo test -p tsinghua_kit` **21 项通过**（`public_api` 新增 `reserves_refuses_unusable_search_text_before_any_session_check`，`compile_reserves_api` 与 `rust_consumers_can_import_curated_domain_modules_without_ffi` 增补馆藏类型）。`cargo fmt --all -- --check` 与 `git diff --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps -p tsinghua_kit` 退出 0；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `ReservesClient` / `ReservesSearch` / `ReservesBook` / `ReservesBookDetail` / `ReservesChapter` / `maxPage == 1000` 与两张列表的不可变。

**同过滤器下的既有失败**：`library` 过滤器仍为 87 通过 / 4 失败，与本轮之前记录的**同一批 4 项**（`backend_repair_cached_library_read_uses_existing_expiry_gate`、`backend_repair_proven_library_action_survives_unavailable_info_bootstrap`、`backend_repair_runtime_library_segments_and_seats_reach_safe_dtos`、`backend_repair_binding_library_timestamp_date_must_match_segment_day`），原因是它们直接安装 library 证明却从不填充 `library_seat_section_ids`，与新增读路径无关；本轮**未修**。`tsinghua_kit_ffi` 的 `classroom_contract` 集成测试 3 项失败（`MissingDateHeaders`）经 `git stash` 对照确认为 **HEAD 既有**，同样不是本轮引入。

**baseline**

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新到第八版：`source_revision` 前进到 `1748bad`；`root_public_modules` 66 → **67**（新增 `reserves_read`）；根 `pub use` 53 → **54**；`rendered_crate_root_item_counts` struct 323 → **337**、enum 155 → **160**、fn 59 → **62**、constant 55 → **60**（`trait` / `type` 不变），与本模块 14/5/3/5 个公开项一一对应；runtime `dto_structs` 76 → **78**、`public_methods` 102 → **104**、`direct_state_field_count` 120 → **123**（新增 `reserves_adapter` / `reserves_proof` / `last_reserves_failure_code`）、`line_count` 28455 → **28740**（与 `git diff --numstat` 的 `285 0` 一致）。方法行号按源码重新锚定：`impl` 之前的插入使既有 102 条整体下移 54 行，两个新方法插在 `load_sports_records_result` 之后（`load_invoice_list_result` / `load_invoice_document_result` / 银行与研究生收入 / 体育两读各下移 54 行，其余 96 条各下移自己的插入偏移）。

**未验证**：本域的线上可用性**未验证**，需要另行真实只读验收——本轮**未对任何真实账号发起任何请求**，也未尝试教参全文阅读（那需要本引擎刻意不实现的校园身份登录）。在此之前不得用 fixture 或空结果冒充线上证据。

## 67. 2026-09-30 研读间（图书馆研读间 / CAB）的房间目录与本人预约（只读）

计划阶段 3 的第四个子域：**研读间**（图书馆座位预约系统的"研读间"应用，参考库里叫 `cab`）。它落在自己的校园主机上，走一个固定的 WebVPN 映射，复用的是 transport 已经持有的 INFO/WebVPN 会话。本子域**只读**，且**没有新增任何认证方式**（`Service::LibraryRoom` 早已存在于 `error_sdk.rs`，本轮只是第一次被真正使用）。

**允许名单条目，以及为什么这一条是"有据"的**

与 §66 的教参域同理：映射令牌是固定 ASCII 前缀 `77726476706e69737468656265737421`（即 `wrdvpnisthebest!` 的十六进制）加 AES-128-CFB（密钥与 IV 同为 `wrdvpnisthebest!`，明文为主机名）。把本模块的令牌 `f3f643d22b396a1e6a1b80a29f5d363409e413829737d1` 解码得到的正是 `cab.lib.tsinghua.edu.cn`，参考库自己的常量也带着 `finalAddress=https:%2F%2Fcab.lib.tsinghua.edu.cn`。因此 `map_additional_roaming` 里的新分支把主机写作**有据**而不是推断。方案写作 `https`，与参考库把每条 CAB 路由拼成 `/https/<token>/…` 一致。该分支本身不授予任何东西：共享检查仍然钉住 scheme/port/userinfo/百分号编码，而"已经映射过的输入"必须落在这个模块自己的映射常量之内。

**刻意不实现参考的恢复策略，并且不接受响应选定的登录 app id**

参考库给这个应用登记的策略是 `"cab"`，也就是一次**校园身份登录**：它先取 `…/ic-web/auth/address` 的响应，从里面用 `/\/login\/form\/(.+)$/` 抠出一个载荷，再拿这个载荷去 `id.tsinghua.edu.cn` 取公钥、提交凭据，然后重试。也就是说，那次提交凭据的请求里，**登录应用 id 是响应给的**——这不是一个常量。引擎不实现第二套校园登录，也**不接受由响应选定的身份登录 app id**，因此 `LIBRARY_ROOM_WEBVPN_TARGET = "cab"` 只作为**文档常量**保留，说明"这条恢复路径被刻意没有实现"，它**没有**被登记为漫游 selector（`info_session.rs` 里登记的是**真实映射令牌**，不是这个策略名）。这一点由测试 `the_cab_identity_login_policy_is_never_registered_as_a_roam_selector` 固定。会话过期统一报 `LibraryRoomAdapterError::SessionExpired`，由既有的 INFO 刷新路径处理，与其它 INFO 承载的读取完全一致。

参考的 `LIBRARY_ROOM_USER_INFO_PATH`（`/ic-web/auth/userInfo`）同理：它是参考用来**判断要不要跑上面那次登录**的探针，本模块**永不请求它**（它只出现在常量声明里，请求路径只有目录与预约记录两条）。这一点在常量自己的文档注释里写明，避免后来者以为"少了一条读取"。

**没有 mock，也不把"读不出来"当"空目录"**

页面是 JSON 信封（`{"code": 0, "data": …}`）。参考实现在自己的 mock 上返回内置数据；本模块改成**服务自己的信封就是证据**：`code` 不为 0 一律失败，缺 `data` 一律失败，而不是空结果。空房间列表只能来自"真的带了空数组的信封"，所以"没有可预约房间"不可能由一次解析失败的响应制造出来。

**会话页与"任何 HTML"是两件事**

`get()` 的判定顺序经过一次返工：早期草稿把**任何** HTML 响应都当成 `SessionExpired`，那会让运行时为一个刷新修不了的部署变化去跑 INFO 认证刷新链——正是 AGENTS.md 禁止的"把解析失败当成可恢复的会话问题"。最终顺序是：先读体 → 超长判 `UnexpectedDeployment` → **只把 `campus_html::classify_page` 认出的登录页/超时页判为 `SessionExpired`** → 非 200 判 `HttpStatus` → 其余 HTML 判 `Parse(NotEnvelope)`（→ `library_room_envelope` → `invalid_response`）→ 内容类型不是 JSON 判 `UnexpectedContentType`。测试 `an_html_page_that_is_not_a_login_page_is_not_folded_into_a_session_failure` 把这条钉住。

**服务自己的拒绝与解析失败可区分**

`code != 0` 走 `LibraryRoomParseError::ServiceRejected { code }`，**服务自己的数字状态被保留**，而它附带的文本被丢弃——服务撰写的文字不进错误、不进日志、也不进 DTO。它在 `diagnostic_code()` 里落到 `library_room_rejected` → `ErrorCode::NotAvailable`（"服务可达且回答了，但这个能力对这个账号是关闭的"），而不是被当成网络或解析故障。

**日期窗口在**任何**会话工作之前验证**

`records` 的 `begin`/`end` 是 `YYYY-MM-DD`。运行时在**建立会话之前**先用 `LibraryRoomProfile::standard().records_request(begin, end)` 校验：反向窗口、非法日期、超过 `LIBRARY_ROOM_MAX_WINDOW_DAYS = 31` 天的窗口都被拒为 `library_room_window` → `invalid_input`，因此被拒的参数**不会花掉一次 handoff**，其文本也永远不会变成服务端查询。适配器内部用同一个 `validated_window` 再校验一次，并把日期**重新打印**成自己的形状（所以查询串的字母表只由本模块的常量构成），未补零的日期会被规范化为 `beginDate=2026-09-03` 而不是原样送出。

**基址配置比"能拼出 URL"更严**

`normalize_base_url` 只接受两种路径：空（裸源）或一个**合法的映射根**（`opaque_mapping_root` 用 `safe_webvpn_mapping_id()` 校验固定前缀、长度 64..=96、偶数、纯小写十六进制）。别的任何配置路径一律 `InvalidBaseUrl`——一个配置进来的更深路径是本模块没有授予的权限，放行就会去访问本模块从未记录过的路由。测试 `a_mistyped_mapping_token_is_refused_where_it_is_configured` 覆盖截断、大写、缺前缀三种错令牌。

**一个自己发现并修掉的失败码污染**

`last_library_room_failure_code` 是"上一次失败是什么"的粘性记录，`library_room_failure()` 会优先读它。于是"先送一个被拒的窗口、再让会话检查失败"会把**窗口拒绝的码**报成会话失败，让 `invalid_input` 出现在一次与参数无关的失败上。修法是：两个读入口在**做完参数校验之后、任何会话工作之前**把该字段清空，于是每次读报的失败码一定是这次读自己的。SDK 与桥接各有一条用例把顺序钉住（先断言会话失败、再断言窗口拒绝、最后再断言一次会话失败仍然是会话失败）。

**SDK / FFI / Dart**

- 引擎：新增模块 `library_room_read`（12 struct / 5 enum / 2 fn / 5 const）；`info_session.rs` 新增一条允许名单分支；`api/runtime.rs` 新增 `load_library_room_catalog_result` / `load_library_room_records_result` 两个公开方法与三个状态字段（`library_room_adapter` / `library_room_proof` / `last_library_room_failure_code`）、六个 DTO；`client.rs` 新增 `LibraryRoomClient`（含 `MAX_WINDOW_DAYS`）；`ServiceId::Info` 的失效分支里补上 `invalidate_library_room_session()`。两条读都走"失效 → INFO 刷新 → 重试一次"的既有模式，第二次仍过期就失败。
- SDK：`tsinghua-kit/src/lib.rs` 新增 `pub mod library_room { … }` 再导出块，`client.rs` 新增 `Client::library_room()` 与薄包装 `LibraryRoomClient`。
- FFI：`ClientHandle` 新增 `library_room_catalog_result` / `library_room_records_result`；新增 `LibraryRoomDto` / `LibraryRoomKindDto` / `LibraryRoomCatalogDataDto` / `LibraryRoomCatalogResultDto` / `LibraryRoomMemberDto` / `LibraryRoomRecordDto` / `LibraryRoomRecordsDataDto` / `LibraryRoomRecordsResultDto`。两张含个人数据的 DTO 有**自定义 `Debug`**：成员只印 `name_present`，预约行只印六个 `*_present` 与 `member_count`，名字、房间名与时间一个都不印。FRB 重新生成，生成物未手工编辑。
- Dart：`lib/src/library_room.dart`（part）+ `lib/library_room.dart` 入口 + `lib/tsinghua_kit.dart` 的 `part`/字段；`LibraryRoomClient.catalog()` 与 `records({begin, end})`，`maxWindowDays = 31`。`deviceId` / `minReserveMinutes` / `kindId` 是 `u64` → `BigInt`；`roomCount` 是 `u32` → 普通 `int`（与 `u64` 不同，生成物在每个平台上都是 `int`）。

**验证**

引擎定向（loopback fixture，无任何真实账号 / Cookie / 房间标识）：`cargo test -p tsinghua_kit_engine library_room_tests` **42 项通过 / 0 失败**，覆盖信封形状、字符串化 id、真实空目录、缺 `roomInfos`、缺/空名字、负 id、控制字符、超长字段被拒而不是被截断、服务拒绝带自己的数字、缺 `data`、非信封体、WebVPN 门户页、超时页、空体、记录形状、**成员账号名不进投影记录**（断言 `Debug` 里没有该账号名）、无设备的预约、缺字段、空列表、窗口校验（反向、`2026-02-30`、`today`、带引号的值）、窗口上下界（正好等于上界通过、超一天被拒）、未补零日期被规范化而不是原样送出、目录计划不带查询、两条 loopback 读的路径与查询逐字节形状、不可用窗口不花请求、登录页回答、**非登录页的 HTML 不折叠成会话失败**、服务拒绝、非 JSON 内容类型、500、401、跨源 302 在**被跟随之前**就被拒（且只发出 1 次请求）、同源但映射外的 302、配置路径规范化与错令牌被拒、profile 固定、以及"CAB 身份登录策略从未登记为漫游 selector"和"映射令牌带固定前缀"。

桥接层 `cargo test -p tsinghua_kit_ffi --lib`（含 `--features ffi-bridge` 与不带该特性两种配置）**51 项通过**，其中本轮新增 2 项（`library_room_bridge_debug_omits_names_and_times`、`library_room_refusals_need_no_account_and_no_request`）。SDK `cargo test -p tsinghua_kit` **22 项通过**（新增 `library_room_refuses_an_unusable_window_before_any_session_check`，`compile_library_room_api` 与 `rust_consumers_can_import_curated_domain_modules_without_ffi` 增补该域类型与 `MAX_WINDOW_DAYS == 31`）。`cargo fmt --all -- --check` 与 `git diff --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps -p tsinghua_kit` 退出 0；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `LibraryRoomClient` / `LibraryRoom` / `LibraryRoomKind` / `LibraryRoomCatalog` / `LibraryRoomRecord` / `LibraryRoomMember`、`maxWindowDays == 31`、空目录只由 `roomCount == 0` 得出，以及两张列表的不可变。

**同过滤器下的既有失败**：`library` 过滤器仍为 87 通过 / 4 失败，与本轮之前记录的**同一批 4 项**（`backend_repair_cached_library_read_uses_existing_expiry_gate`、`backend_repair_proven_library_action_survives_unavailable_info_bootstrap`、`backend_repair_runtime_library_segments_and_seats_reach_safe_dtos`、`backend_repair_binding_library_timestamp_date_must_match_segment_day`），原因是它们直接安装 library 证明却从不填充 `library_seat_section_ids`，与新增读路径无关；本轮**未修**。`tsinghua_kit_ffi` 的 `classroom_contract` 集成测试 3 项失败（`MissingDateHeaders`）经 `git stash` 对照确认为 **HEAD 既有**，同样不是本轮引入。整个 `--lib` 引擎套件在本机 HEAD 上还会因一个既有的栈溢出而中断（`backend_repair_learn_fresh_announcement_cache_skips_live_handoff`），因此本轮的判据是**定向**测试而不是整套。

**baseline**

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新到第九版：`source_revision` 前进到 `a51bca4`；`root_public_modules` 67 → **68**（新增 `library_room_read`）；根 `pub use` 54 → **55**；`rendered_crate_root_item_counts` struct 337 → **349**、enum 160 → **165**、fn 62 → **64**、constant 60 → **65**（`trait` / `type` 不变），与本模块 12/5/2/5 个公开项一一对应；runtime `dto_structs` 78 → **84**、`public_methods` 104 → **106**、`direct_state_field_count` 123 → **126**（新增 `library_room_adapter` / `library_room_proof` / `last_library_room_failure_code`）、`line_count` 28740 → **29129**（与 `git diff --numstat` 的 `389 0` 一致）。方法行号按源码重新锚定：本轮在 runtime 里新增的整段使既有 104 条公开方法、6 个公开自由函数与全部 DTO 声明一起下移 **133 行**（不是逐段偏移），两个新方法插在 `load_reserves_detail_result` 之后。

**未验证**：本域的线上可用性**未验证**，需要另行真实只读验收——本轮**未对任何真实账号发起任何请求**，也未尝试预约、取消或联系方式修改（这些路由在本模块里**根本不存在**）。在此之前不得用 fixture 或空结果冒充线上证据。

## 68. 2026-09-30 校园卡自身的状态变更（一次性写：挂失/解挂、交易密码、限额、圈存）

计划阶段 2 的校园卡条目。五条路由全部落在既有的 `card.tsinghua.edu.cn` 上，全部 `POST` + `Content-Type: application/json`，**没有任何 CSRF 字段**（服务只认会话 Cookie）。它们与既有的只读半（账户 / 流水）共用同一个 `CampusCardClient` 和同一份 card SSO 会话，因此**未新增 selector、未新增映射、未新增 `ServiceId`、未新增认证方式**：`Service::CampusCard` 早已存在，本轮只是第一次用到它的写半。

**路由与线格式（全部来自参考实现的观测值）**

| 操作 | 路由 | 体字段 |
| --- | --- | --- |
| 挂失 | `POST /business/cardReportLoss` | `idserial`, `txpasswd` |
| 解挂 | `POST /business/solutionHang` | `idserial`, `txpasswd` |
| 交易密码修改 | `POST /business/modifyPwdByPhoneVerify` | `idserial`, `oldpassword`, `txpassword`, `authOldPwd: true` |
| 限额修改 | `POST /business/modifyCardMaxConsamt` | `maxconsamt`, `maxconstolamt`, `txpassword`, `cardid` |
| 圈存充值 | `POST /business/moblieRecharge` | `idserial`, `txamt`（分） |

注意三条与直觉相反、因此必须在代码里写死的观测结果：`txpasswd` 的拼写就是 `txpasswd`（不是 `txpassword`）；交易密码修改的路由名里带 `PhoneVerify`，但**参考实现一个验证码都没发**（`authOldPwd: true` 表示用旧密码自证）；圈存的观测实现接受一个 `transactionPassword` 参数，但**它从不进入请求体**——这是个死参数，本模块因此也只有一个入参。

**两个限额字段保留线名，不猜含义**

参考实现的读半与写半对 `maxconsamt` / `maxconstolamt` 的配对是**相反的**：参考写半把参数的"日限额"填进 `maxconsamt`、"单次限额"填进 `maxconstolamt`，而引擎自己的读解析器（`campus_card_read.rs`）把 `maxconstolamt` 读作日限额、`maxconsamt` 读作单次限额。两半不可能同时对，且没有第三份证据能判定谁被转置了。因此本模块的 API **直接以线字段命名参数**（`maxconsamt_cents` / `maxconstolamt_cents`），在文档里写明两半分歧，并**拒绝**给它们安上"日"/"单次"的含义。测试 `the_limit_plan_sends_the_wire_fields_in_the_observed_pairing` 把这一对钉死。

**支付码那一条：记录为常量，但不成为操作**

校园卡**没有**"支付码"字段。参考实现的另一个充值入口 `POST /wx/rechard/qrcode` 返回 `bizContent.webUrl`，是一个一次性支付链接；而它还被一个**版本探针**挡在前面（`app.cs.tsinghua.edu.cn/api/CardIVersion`，版本 ≤ 2 才走），那个后端属于 App 专属域，本仓库刻意不实现。于是 `CARD_QR_TOPUP_PATH` 只作为**文档常量**保留，**没有任何操作**的 `path()` 能等于它，测试 `the_qr_topup_route_is_recorded_but_is_not_an_operation` 断言这一点。这也是"本层不存在任何能产出支付码的调用"的结构性保证，而不是一句注释。

**密码不出模块**

`CampusCardSecret` 包着 `Zeroizing<String>`：构造时拒绝空/纯空白/超长（> `MAX_CARD_SECRET_CHARS = 64`）/含控制字符，**不做 trim**（trim 会把一个不同的密码悄悄变成一个正确的密码）；`Debug` 只打印 `chars: N`；`Drop` 显式 `zeroize()`；`expose()` 是 `pub(crate)`。限额计划里的 `Debug` 只打印线字段名与 `has_relative_path`。因此密码既不可能出现在日志里，也不可能出现在 DTO、错误或 FFI 边界上。

**"恰好发一次" 的三层落点**

1. **适配器层**：`execute_write` 经 `transport.execute_once_exclusive(...)` **显式取整个 gate、不跟随重定向**——一次重定向就是一次"结果已不确定的写"的重放。传输失败被归类为 `Unrecognized`，**错误文本被丢弃**（reqwest 的失败信息可能带上请求 URL，而这次请求的体里装着密码）。
2. **分类层**：`Accepted` 是唯一成功；`Refused`（服务自己的失败包，包括 AES 加密的失败回退）是**确定的拒绝**，映射到 SDK 的 `AuthenticationRejected`（"什么都没变，输入不对"），与 `OutcomeUnconfirmed`（"已经发出去了，效果未知，去看卡的状态"）是**不同的码**，调用方不需要读任何文案就能区分。`LoginRequired`（401/403、登录重定向、`classify_page` 认出的登录/超时页、体里的会话标记）作废 card 会话并记 `card_write_session_expired`。其余（重定向状态、非 2xx、路径不符、越界重定向、空体、超限、非 JSON）一律 `Unrecognized`。
3. **绝不自动重认证后重发**：这是校园卡写与图书馆座位预约**读**的刻意差别。一次已经发出的状态变更，其回答丢失后再发一次就是重放；本模块因此在这个分支上**只报错不重试**。

**卡号由 Rust 自己读，绝不是参数**

限额路由还需要 `cardid`。它不是任何一层的参数：`read_card_id` 用已证明的 card 会话重读一次账户，并且**用 `parse_card_account_response(..., Some(&session_account))` 把整份账户响应按会话账号重新验一遍**，只有账号一致才取第一张卡的 `cardid`。测试 `card_write_refuses_a_response_for_another_account` 断言返回 `AccountMismatch` 且**一个写请求都没发**。

**本地边界与它们的来源**

- `MIN_CARD_TOPUP_CENTS = 1_000` / `MAX_CARD_TOPUP_CENTS = 20_000`：来自参考实现自己的输入校验（金额至多两位小数、10..200 元），是按调用方的规则而不是服务端规则，本模块照样执行，免得一次转账发出没人会发的金额。
- `MAX_CARD_LIMIT_CENTS = 100_000_000`：**本模块自己的**上界。服务端上界没有观测到（读半只报当前限额，不给范围），存在只是为了挡住手滑。
- 三者都在**任何请求存在之前**生效（`InvalidAmount` ⇒ SDK `invalid_input`），因此被拒的参数不花掉任何一次 dispatch。

**SDK / FFI / Dart**

- 引擎：新增 `campus_card_write`（10 常量 / 1 struct / 6 enum / 1 fn，11 项单元测试）；`campus_card_read` 新增 `first_card_id`；`campus_card_adapter` 新增 `Refused` 变体、`read_card_id`、`execute_write`、`classify_card_write` 与 6 项 loopback 测试；`api/runtime_campus_card_write.rs` 新增 `ensure_campus_card_write_session`（证明 Identity + CampusCard 两门服务证明、registry 用户相等、且卡的 cookie jar 与身份 transport **是同一个 Arc**）/ `finish_card_write` / `apply_card_write`；runtime 新增 `apply_campus_card_write`、`last_campus_card_failure_code` 字段与两个访问器。
- SDK：`CampusCardClient::apply_write(&CampusCardWriteRequest)`，`campus_card` 模块 re-export `CampusCardWriteRequest` 与六个路径/边界常量；`public_api.rs` 新增 `compile_campus_card_write_api` 编译检查。
- FFI：`ClientHandle` 新增 `campus_card_report_loss` / `campus_card_cancel_loss` / `campus_card_change_transaction_password` / `campus_card_modify_spending_limit` / `campus_card_top_up_from_bank`；FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/campus_card.dart` 新增五个方法与 `minTopUpCents` / `maxTopUpCents` / `maxLimitCents`；新增 `lib/src/int64.dart` 提供 `platformInt64FromBigInt`（桥接的 `i64` 在 native 上是 `int`、在 web 上是 `BigInt`，这个转换对超出 64 位的值**抛错而不是截断**）；`lib/campus_card.dart` 入口描述从"只读"改为"账户与流水访问，外加卡自身的状态变更"。

**验证**

引擎定向（loopback fixture，无任何真实账号 / Cookie / 卡号）：`cargo test -p tsinghua_kit_engine --lib campus_card` **61 项通过 / 0 失败**，其中本轮新增 17 项（11 项模块单元测试 + 6 项 loopback）：密码类值的拒绝与脱敏、计划 `Debug` 只印线名与 `has_relative_path`、五条路由都不带查询、QR 路由永不成为操作、限额字段的线配对、挂失/解挂用 `txpasswd` 而改密用 `authOldPwd`、圈存体不带密码、越界金额在计划之前被拒、只有服务自己的错误令牌算拒绝、显式失败标记可识别；loopback 侧覆盖"恰好发一次且只发那一条路由"、"拒绝是确定的而传输失败不是"、"不可读答复判为未确认且不重发"、"会话消失时报告而不是重试"、"另一个账号的响应被拒且零写请求"（内联形状的 `Debug` 里没有密码、也没有服务文案）。桥接层 `cargo test -p tsinghua_kit_ffi --lib` **53 项通过**，本轮新增 2 项（7 种非法输入在**任何会话存在之前**被判 `invalid_input`；一个形状合法的圈存在**没有会话**时报 `session_required` 而不是 `outcome_unconfirmed`）。SDK `cargo test -p tsinghua_kit`（含 `public_api` 22 项、`client_api` 12 项）全部通过。`cargo check --workspace --all-targets` 退出 0；`cargo fmt --all -- --check` 干净；`git diff --check` 干净；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `CampusCardClient` / 三个本地常量 / 空的卡号引用语义。

**同过滤器下的既有失败**：`library` 过滤器仍为既有的 4 项失败，`tsinghua_kit_ffi` 的 `classroom_contract` 集成测试 3 项 `MissingDateHeaders` 失败，均为 HEAD 既有、与本轮无关（见 §64 与 §67 的归因），本轮**未修**。整套 `--lib` 引擎运行仍会因既有的栈溢出中断（`backend_repair_learn_fresh_announcement_cache_skips_live_handoff`），因此判据是**定向**测试。

**baseline**

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新到第十版：`source_revision` 前进到 `48c81ef`（`library_room` 那一次的提交 48c81ef 之后，本轮改动尚未提交）；`root_public_modules` 68 → **69**（新增 `campus_card_write`）；根 `pub use` 55 → **56**；`rendered_crate_root_item_counts` struct 349 → **352**、enum 165 → **171**、fn 64（不变）、constant 65 → **75**（十四个新公开项在 crate 根渲染出 3 struct / 6 enum / 10 constant），`trait` / `type` 不变；runtime `public_methods` 106 → **107**，`direct_state_field_count` 126（本轮之前的实测值即为 126，新增一个字段，因此该记录项在 `library_room` 那次已经计过；本版按当前源码**逐条重新锚定全部 107 条方法的行号**，因为 `apply_campus_card_write` 的插入使其后所有方法下移），`line_count` 29129 → **29183**；runtime `dto_structs` 84（本轮无新 DTO）。

**未验证**：五条路由的线上可用性**未验证**，需要另行真实只读验收；本轮**未对任何真实账号发起任何请求**，也未挂失、解挂、改密、改限额或转账。按 §59 起的约定，这些写操作**一律不进入只读验收**，只在其结果上做"是否未确认"的判定。在此之前不得用 fixture 或空结果冒充线上证据。

## 69. 2026-09-30 宿舍电费密码重置（一次性写：宿舍服务自身的口令替换）

计划阶段 2 的宿舍密码条目。参考实现 `thu_reference/thu-info-app/packages/thu-info-lib/src/lib/dorm.ts` 里 `resetDormPassword` 是这样一个调用：

```ts
roamingWrapperWithMocks(helper, "id", "051bb58cba58a1c5f67857606497387f", async () => {
    const $ = await uFetch(CHANGE_HOME_PASSWORD_URL).then(cheerio.load);
    if ($("#ChangePasswordCtrl1_txtoldpassword").length === 0) throw new DormAuthError();
    ...
    form.__EVENTTARGET = "ChangePasswordCtrl1:btnOK";
    form.ChangePasswordCtrl1$txtoldpassword = "";
    form.ChangePasswordCtrl1$txtnewpassword = newPassword;
    form.ChangePasswordCtrl1$txtnewpassword1 = newPassword;
    await uFetch(CHANGE_HOME_PASSWORD_URL, form);
});
```

也就是说它的**漫游策略是 `"id"`**，而不是电费读半用的那条策略。`"id"` 在本仓库里是一条已经带结论的边界：它是一次**校园身份登录**，会把账号口令 POST 到 `id.tsinghua.edu.cn`。§58 的研读间（`"cab"`）、§67 的馆藏教参（同样是 `"id"`）都因为同一条理由没有被实现成第二条登录。本轮沿用同一结论：**不实现第二条校园登录**。

**为什么这条仍然能实现：映射根是同一个**

参考库的 `CHANGE_HOME_PASSWORD_URL` 与电费剩余量 URL 在 `constants/strings.ts` 里只差最后一段路径，前缀是**同一个映射 token**：

```
https://webvpn.tsinghua.edu.cn/http/77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9/Netweb_List/ChangePassword.aspx
```

而引擎的 `ELECTRICITY_WEBVPN_BASE_URL`（`api/runtime.rs`）带的正是同一个 token `fdee49932a3526446d0187ab9040227bca90a6e14cc9`。因此 `configured_electricity_flow()` 已经解析出来的那个映射根，就是这张表单所在的映射根——宿舍电费页与宿舍改密页是**同一个遗留 ASP.NET 应用**的两个页面。于是本模块不新增 selector、不新增映射、不新增 `ServiceId`、不新增认证方式、不新建 Cookie jar：它复用**读半已经证明过的那一条电费会话**与同一个 `runtime.identity.transport()`。这一点在 `runtime_dorm_password_write.rs` 里是结构性的（适配器就是从 `configured_electricity_flow().mapped` 与 `runtime.identity.transport()` 构造的），不是注释承诺。

`DORM_CHANGE_PASSWORD_WEBVPN_TARGET`（`051bb58cba58a1c5f67857606497387f`）作为**文档常量**保留：它记录的是"这个路由自己的客户端用哪条策略漫游"，而那条策略本仓库不实现。它与 §66 记录的 `"id"` 常量一样，**没有**出现在 `info_session.rs::map_additional_roaming` 的允许名单里，因此它不可能是任何一次请求的目的地。

**线格式：三个钉死的观测值**

| 事实 | 值 | 为什么必须写死 |
| --- | --- | --- |
| 就绪锚点 | `id="ChangePasswordCtrl1_txtoldpassword"` | 参考实现以它缺失判定 `DormAuthError`（即会话没了），本模块同样以 `FormMissing` 报告 |
| 事件目标 | `__EVENTTARGET = "ChangePasswordCtrl1:btnOK"` | 分隔符是**冒号**；同页隐藏字段的 `name` 用的是 `$`（`ChangePasswordCtrl1$txtoldpassword`）。写成 `$` 会指向不存在的控件 |
| 旧密码字段 | `ChangePasswordCtrl1$txtoldpassword = ""` | 服务自己的客户端**主动送空**，不是省略。本模块照作，并且因此**不接受**任何旧密码参数 |

新口令与确认字段送同一个值（`$txtnewpassword` / `$txtnewpassword1`），与参考一致。

**表单状态属于服务，不属于调用方**

`parse_change_password_form` 只从这一会话真正取回的那张页面里收集 `<input type="hidden">` 的 name/value（跳过无 name 的、去重、有界：`MAX_FORM_FIELDS = 512` / `MAX_FORM_VALUE_BYTES = 256 KiB` / `MAX_FORM_RESPONSE_BYTES = 512 KiB`），产出 `DormPasswordFormState`。POST 体 = 这张页面的隐藏字段**原样回显** + 本模块设置的四个字段。计划（`DormPasswordWritePlan`）**不携带表单**，所以调用方无法替换请求体：`DormPasswordWritePlan::fields()` 是 `pub(crate)`，且把口令放在**派发时刻**才拼进去（`.form(&fields).build()` 之后立刻 `drop(fields)`）。

**没有 `Refused`，这是刻意的**

校园卡写（§68）有 `Refused`，因为参考实现观测到了服务自己的失败包。这条路由**没有任何响应被观测过**：参考实现 `await uFetch(...)` 后**丢弃返回值**，只有异常与成功两种外部表现。因此 `DormPasswordWriteOutcome` 只有 `Accepted` / `LoginRequired` / `Unrecognized` 三态，**没有** `Refused`。一个带显式失败标记的 JSON 信封也判为 `Unrecognized`（未确认），而不是"被拒绝"——凭空造一个拒绝态等于把"我不知道"说成"你没改成"。

`classify_dorm_password_write` 的接受面因此收得很紧：**空体/纯空白**算接受（一次成功的 ASP.NET 回发常常什么都不返回）、字面 `OK` 算接受、JSON 对象里 `status`/`result`/`success` 为 `1`/`true`/`"success"` 且无失败标记算接受；**HTML 页面一律不算接受证据**（这里用的是严格版 `looks_like_html`，与表单解析器用的宽松版 `looks_like_html_body` 是两个函数——前者拒绝把"另一个页面"当成成功，后者只是允许 `<%@ Page %>` 开头的页面被解析）。其余全部 `Unrecognized`。

**三层"恰好发一次"**

1. **适配器**：`reset_password` 走 `transport.execute_once_exclusive(...)`——占满 gate、**不跟随重定向**。一次重定向就是一次"结果已不确定的写"的重放。
2. **错误文本被丢弃**：`Err(_error) => Ok(Unrecognized)`。reqwest 的失败信息可能带上请求 URL，而这次请求的体里装着新口令，所以失败信息**不进日志、不进错误、不进 DTO**。
3. **runtime 不重试、不自动重认证**：`LoginRequired` 只作废电费会话并记 `dorm_write_session_expired`；其余未确认记 `dorm_write_unconfirmed`。

**未证明的会话绝不留写失败码**

`apply_password_reset` 在**做任何事之前**检查 `electricity_service_is_proven()`，不成立时返回一句 `record_error("宿舍服务会话未建立，请先打开宿舍电费页面")`，不落业务失败码。因此一个**根本没发出去**的请求在 SDK/FFI 层看到的是 `session_required`，而**不是** `outcome_unconfirmed`。这是"未确认 = 可能已经改了"这一语义的结构性保证。

**本地边界与它的来源**

`MAX_DORM_PASSWORD_CHARS = 64` 是**本模块自己的**上界：服务端规则未被观测，它存在只是为了挡住手滑。除长度外，`DormPassword::new` 还拒绝空串、纯空白、任何控制字符，并**不做 trim**（trim 会把一个不同的口令悄悄变成一个正确的口令）。`DormPassword` 包着 `Zeroizing<String>`，`Debug` 只打印 `chars: N`，`Drop` 显式 `zeroize()`，`expose()` 是 `pub(crate)`。越界值在任何请求存在之前被拒（`dorm_write_request` ⇒ SDK `invalid_input`）。

**SDK / FFI / Dart**

- 引擎：新增 `dorm_password_write`（8 常量 / 6 struct / 7 enum / 2 fn，13 项模块单元测试）；`api/runtime_dorm_password_write.rs` 新增 `apply_password_reset` / `finish_dorm_password_reset`；runtime 新增 `apply_dorm_password_reset`、`last_dorm_password_failure_code` 字段与两个访问器；`client.rs` 新增 `ElectricityClient::reset_home_password` 与 `dorm_password_error_code` 映射（`dorm_write_unconfirmed` ⇒ `OutcomeUnconfirmed`、`dorm_write_session_expired` ⇒ `SessionExpired`、`dorm_write_request` ⇒ `InvalidInput`、`dorm_config` ⇒ `Unsupported`、`dorm_network` ⇒ `NetworkUnavailable`、表单/来源/路径/内容类型/HTTP ⇒ `InvalidResponse`）及其 1 项单元测试。
- SDK：`ElectricityClient::reset_home_password(&mut self, new_password: &str)`（薄包装）；`electricity` 模块 re-export `DORM_CHANGE_PASSWORD_PATH` / `DORM_CHANGE_PASSWORD_ANCHOR` / `MAX_DORM_PASSWORD_CHARS`；`public_api.rs` 新增一条断言（`Service::Dorm` + 已登出时 `SessionRequired`，四种非法值 `InvalidInput`）。
- FFI：`ClientHandle::electricity_reset_home_password(new_password: String)`；FRB 重新生成的生成物未手工编辑。
- Dart：`lib/src/electricity.dart` 新增 `resetHomePassword` 与 `maxHomePasswordChars`；`lib/electricity.dart` 入口描述补充这一次性写与其 `OutcomeUnconfirmed` 语义；`test/public_entrypoints_test.dart` 覆盖 `maxHomePasswordChars == 64`。

**验证**

引擎定向：`cargo test -p tsinghua_kit_engine --lib dorm_password` **14 项通过 / 0 失败**（13 项 loopback/单元 + 1 项错误码映射），覆盖：口令值的拒绝与脱敏、计划 `Debug` 只印字段名与 `fields` 数量、旧口令字段送空而新口令重复两次、表单解析回显每个隐藏字段并容忍页面指令开头、非本表单的页面被拒、分类器只接受肯定证据、**恰好发一次**且 POST 体里带页面自己的 `__VIEWSTATE` 与冒号事件目标、不可读答复判为未确认、会话消失时不发请求、页面失去表单时不发请求、登录重定向判为会话失效、空表单被接受而失败状态判为未确认、越界 base URL 被拒。桥接层 `cargo test -p tsinghua_kit_ffi --lib` **55 项通过**（本轮新增 2 项：无会话时报 `session_required` 而非 `outcome_unconfirmed`；四种非法输入在任何会话工作之前判 `invalid_input`）。SDK `cargo test -p tsinghua_kit --test public_api` **23 项通过**。`cargo check --workspace --all-targets` 退出 0；`cargo fmt --all -- --check` 干净；`RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 干净；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过。

**同过滤器下的既有失败**：`library` 过滤器仍为既有的 4 项失败，`tsinghua_kit_ffi` 的 `classroom_contract` 集成测试 3 项 `MissingDateHeaders` 失败，引擎 rustdoc `-D warnings` 仍有 4 条既有私有链接错误（`assessment_read`/`invoice_read`/`library_room_read`/`reserves_read`），整套 `--lib` 引擎运行仍会因既有栈溢出中断（`backend_repair_learn_fresh_announcement_cache_skips_live_handoff`）——均为 HEAD 既有、与本轮无关（见 §64/§67/§68 的归因），本轮**未修**。

**baseline**

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新到第十一版：`source_revision` 前进到 `2e3f62c`（§68 那一次提交；本轮改动尚未提交）；`root_public_modules` 69 → **70**（新增 `dorm_password_write`）；根 `pub use` 56 → **57**；`rendered_crate_root_item_counts` struct 352 → **358**、enum 171 → **178**、fn 64 → **66**、constant 75 → **83**（本模块 23 个公开项在 crate 根渲染为 6 struct / 7 enum / 8 constant / 2 fn），`trait` / `type` 不变；runtime `public_methods` 107 → **108**（新增 `apply_dorm_password_reset`，行号 8248；因该方法的插入，其后的方法行号统一下移 36 行，本版**逐条重新锚定全部 108 条**并同时修正了 `public_free_functions` 6 条的旧行号），`direct_state_field_count` 126 → **128**，`line_count` 29183 → **29232**；runtime `dto_structs` 84（本轮无新 DTO）。

**未验证**：该路由的线上可用性**未验证**，需要另行真实只读验收；本轮**未对任何真实账号发起任何请求**，也**未修改任何账号的口令**。按 §59 起的约定，密码重置**一律不进入只读验收**，只在其结果上做"是否未确认"的判定。参考实现的 `"id"` 漫游策略（一次真正的校园身份登录，会把账号口令 POST 到 `id.tsinghua.edu.cn`）**已记录、未实现**。在此之前不得用 fixture 或空结果冒充线上证据。

## 70. 2026-09-30 体育场馆的写入半（一次性写：下单、退订；验证码是普通读）

计划阶段 5 的第二半。§61 已经补上体育场馆的**只读**半（场地资源与预约记录），但当时把写操作整片排除在模块之外（"不可达比被守卫更强"）。本节把这一片补上，同时**保留**两条边界：支付链仍然不可达，联系电话更新**没有**跨过桥。

**参考实现的三条写路由**

`thu_reference/thu-info-app/packages/thu-info-lib/src/lib/sports.ts` 里三条路由的观测形态是：

```ts
// 下单：不是表单，而是一个扁平的 JSON 对象，键是部署自己的名字
const orderResult = await uFetch(SPORTS_MAKE_ORDER_URL, {
    "bookData.totalCost": totalCost,
    "bookData.book_person_zjh": "",
    "bookData.book_person_name": "",
    "bookData.book_person_phone": phone,
    "bookData.book_mode": "from-phone",
    "gymnasium_idForCache": gymId,
    "item_idForCache": itemId,
    "time_dateForCache": date,
    "userTypeNumForCache": 1,
    "putongRes": "putongRes",
    "code": captcha,
    "selectedPayWay": 1,
    "allFieldTime": `${resHashId}#${date}`,
}).then(JSON.parse);
if (orderResult.msg !== "预定成功") { throw new SportsError(orderResult.msg); }

// 退订：体里只有一个字段
await uFetch(SPORTS_UNSUBSCRIBE_URL, {bookId});

// 联系电话：**体是空的**，值全在 query 里，最后一个是账号自己的登录 id
await uFetch(`${SPORTS_UPDATE_PHONE_URL}${phone}&gzzh=${helper.userId}`, {});
if (response.includes("找回密码")) { throw new LibError(); }
```

三条路由都在**既有的** venue 映射（`a5a70f88…`）与**既有的** selector（`5539ECF8CD815C7D3F5A8EE0A2D72441`）上——`roamingWrapperWithMocks(helper, "default", …)` 与只读半用的是同一个策略名。因此本轮**没有**新增 selector、映射、主机、`ServiceId`、认证方式或 Cookie jar：写半与只读半共用 `ensure_info_session` + `additional_roaming` 已经证明过的那一条会话。

**线格式：四处与直觉相反、因此写死在代码里的观测值**

| 事实 | 值 | 为什么必须写死 |
| --- | --- | --- |
| 下单体不是表单 | 扁平的 13 个键，其中四个带 JavaBean 前缀（`bookData.totalCost` / `bookData.book_person_zjh` / `bookData.book_person_name` / `bookData.book_person_phone`） | 它**不是** `<form>` 的字段集合，用页面元素去读会一个都读不到。`ORDER_FIELD_*` 十三个常量钉住的就是这个集合，一个不多一个不少 |
| 两个"人物"字段是**空串** | `bookData.book_person_zjh` 与 `bookData.book_person_name` 送 `""` | 观察到的客户端**主动送空**，不是省略。本模块照作，并且因此**不接受**任何"预约人"参数：预约人只能是账号自己 |
| 时段令牌与日期是拼接的 | `allFieldTime = "{resHash}#{date}"`，分隔符是 `#` | 参考实现把它拼在请求体里；写成两个字段会指向一个不存在的输入 |
| 事件/账号参数名 | 联系电话路由的账号参数叫 `gzzh`，其值是**账号自己的登录 id** | 它由 runtime 从已证明的身份派生（与 §57 课程成绩的 `XH` 同一条规则），**不是**调用方参数、不是结果字段、不是日志字段 |

**支付链：记录而绝不请求**

`ms=newPay` / `ms=newPayForLater` → `zjjsfw` 映射（`f6f60c93…`）→ `check.do` + `webPay.do` → `generalGetPayCode`。它在 `sports_write` 里**只有常量**：`SPORTS_PAYMENT_MAPPING_TOKEN`、`SPORTS_PAYMENT_HOST`、`SPORTS_MAKE_PAYMENT_PATH`、`SPORTS_PAYMENT_CHECK_PATH`、`SPORTS_PAYMENT_ACTION_PATH`。四条独立的理由，第一条单独就足够：

1. **它唯一的产物是支付码。** 整条链终于 `generalGetPayCode`，它从支付页读 `input[name=qrCode]` 并返回该值的最后一段路径——一次支付的一次性持有者令牌。支付码不得进 DTO、日志或台账，而这条路由不产出别的东西，实现它等于交给调用方一个无法收尾的调用（与 §68 记录 `CARD_QR_TOPUP_PATH` 时完全同一条理由）。另需注意 `ms=newPay` **本身就是资金动作**，不是什么"无害的前半段"。
2. **它会为第二个主机再登记一次映射。** 那需要在 `info_session` 的允许名单里加一条**只为了携带一个支付凭证**而存在的臂。
3. **观察到的客户端自己这一步就不可靠。** 它 POST 到 `paymentResultForm.attr()!.action`，而参考自己的注释写着 `attr()` 返回 `undefined`——本引擎要复现的这一步，参考自己都不信。
4. **取令牌那一步依赖重定向的 method 降级。** 观察到的传输在 303（以及 POST 后的 301/302）上丢弃方法，令牌正是这样到达 `webPay.do` 的。本模块的独占派发**不跟随重定向**，等价的一步只能是一次显式的第二次 GET——那将是本模块的发明而不是观测。

测试 `backend_refactor_sports_write_never_reaches_the_payment_chain` 钉住的是"任何计划都无法指向这几条路径"，包括一个**未来**的调用方也无法构造（计划只能由 profile 的三个构造函数产出，三者的 path 都是常量）。

**联系电话更新：模块里有，桥上没有**

`SportsWriteProfile::update_phone_request` 与 `SportsWriteAdapter::update_phone` 是**完整的实现**（含 query 拼接、账号 id 校验、空体、`looks_like_html` 会话判定），有 3 项测试覆盖它，但**没有任何 runtime 方法调用它**：`ClientHandle` 上没有对应方法、SDK 上没有、Dart 上没有。这是一条**有意的收窄**，理由与支付链不同：

- 它的**唯一输入是手机号**，而号码在账本里属于个人信息。§61 已经为只读半定下"手机号只读、只解码、不进 `Debug`"的规则；把它变成**可写**意味着这个域第一次有了"接受一个个人信息作为调用方参数"的方法，而它的收益（在 App 里改场馆联系电话）本轮没有产品侧需求，风险（App 侧多一条个人信息流）是确定的。
- 它**没有**可用于判定的拒绝措辞：参考实现的唯一检查是"响应里包含 `找回密码`"，那是登录页标记，落在共享的登录分类器里，因此这条路由**只能**是 `Accepted` / `LoginRequired` / `Unrecognized`。一个没有"服务说不"的能力，在桥上有害无益。

因此 `SportsWriteOperation::UpdatePhone` 存在、被测试、被文档记录，但**不可达**；`SPORTS_UPDATE_PHONE_QUERY_PREFIX` 与 `SPORTS_UPDATE_PHONE_ACCOUNT_PARAM` 作为常量导出，供将来的一轮直接使用。

**验证码是一次普通读**

`SportsCaptcha`（`content_type` + 有界 raster 字节）经 `SportsWriteAdapter::read_captcha` 取得，走的是**普通 `send`** 而不是独占派发——它不改变任何状态，所以可以重复：一个看不清图的人再要一张是正常行为。观察到的客户端自己给 URL 加了一个 `Math.floor(Math.random() * 100)=` 的缓存破坏参数，本模块照作（用时间戳的次秒位，不是随机数——它只是让代理不去拿旧图，不是凭证）。**不是**图片的 200 响应（登录页、错误文档、声明为 `text/html` 的字节）是错误而不是空图，与 §61 只读半的分类一致。

**"恰好发一次"的三层**

1. **适配器**：三条写都经 `transport.execute_once_exclusive(...)`——占满 gate、**不跟随重定向**、返回第一个响应。`dispatch` 里对一个未跟随的重定向、一个非 2xx、一个路径/ query 与预期不符的响应统一返回 `Ok(None)`，调用方一律判为 `Unrecognized`。
2. **错误文本被丢弃**：`Err(_error) => Ok(None)`。reqwest 的失败信息可能带请求 URL，而下单路由的**体**里装着一次性 booking hash、联系电话路由的 **query** 里装着账号登录 id 与联系电话，所以失败信息不进日志、不进错误、不进 DTO。
3. **runtime 不重试、不自动重认证**：`LoginRequired` 只作废场馆会话与 INFO 会话并记 `sports_write_session_expired`；其余未确认记 `sports_write_unconfirmed`。这与只读半是**结构性差别**：只读半会在 session expired 后 `refresh_nonacademic_service_after_expiry` 再读一次，写半**绝不**——重认证后重发一次效果未知的状态变更就是重放它。

**句柄的来源与控制权的消耗**

`load_sports_resources_result` 在成功时调 `confirm_sports_slots`：只为 `can_net_book == true` 且带 `res_hash` 的时段铸一个 UUID selector，并把**同一个读**里的 `res_hash` / `cost` / `gym_id` / `item_id` / `date` 一起存进 `ConfirmedSportsSlot`；`load_sports_records_result` 通过 `confirm_sports_reservations` 只为带 `book_id` 的行铸 selector。两个家族与新闻订阅、图书馆座位共用同一条活性规则 `subscription_rule_is_live`：**owner 相同 AND `at.elapsed() < 300s`**。

`book_sports_slot` / `cancel_sports_reservation` 在成功之后 `remove(&selector)`：场馆确认之后这个句柄就退休，同一个句柄不能再发第二次（验证码无论如何都是一次性的）。桥接层更进一步：`ClientHandle` 在派发**之前**就 `remove()` 掉自己的 `SportsSlotRef` / `SportsReservationRef`，因此一个未确认的结果**结构上不可能**被同一个句柄重放。两层是防御纵深，不是重复：runtime 的 map 是 provenance 的唯一权威（账号 + 读的归属），FFI 的 map 只是上下文守卫。

未确认时句柄**不**退休——这是刻意的：一个答案丢失的下单可能已经生效，也可能没有，而无论哪种情况调用方都只能靠**重新读 `records()`** 定论；把句柄留着并不能让它重发（FFI 层已经消耗了它），只是不假装知道结果。

**拒绝与未知是两码事，而且只有一条路由有"拒绝"**

`SportsWriteOutcome` 四态：`Accepted` / `Refused` / `LoginRequired` / `Unrecognized`。只有下单路由会产生 `Refused`，因为只有它印了自己的 `msg`（`classify_order_answer` 只接受 `预定成功` 为接受，其余可读的 `msg` 是拒绝）。退订与联系电话路由**从未被任何客户端观测到拒绝措辞**，因此 `classify_unconfirmed_only` 只承认肯定证据（空体、`OK`、`{"status"|"result"|"success": 1|true}` 且无失败标记），其余一律 `Unrecognized`：凭空造一个拒绝态等于把"我不知道"说成"你没退成"。

**本地边界与它们的来源**

| 边界 | 值 | 来源 |
| --- | --- | --- |
| `MAX_SPORTS_CAPTCHA_CHARS` | 12 | 本模块自己的上界：验证码是给人读并敲进去的，太长是调用方错误 |
| `MAX_SPORTS_HASH_CHARS` | 64 | 观测到的 hash 长度 48，留出余量；与只读半的解析上界一致 |
| `MAX_SPORTS_RECEIPT_CHARS` | 32 | 只被记录下来的支付链消费（`VALID_RECEIPT_TITLES` 三个值都远短于此） |
| `SPORTS_MAX_SINGLE_PAYMENT_COST` | 42 | **观察到的客户端自己的**常量（参考的预约页拒绝 42 以上的单笔支付并给出"出于安全考虑…单笔金额不得超过 42 元"）。保留它，因为观察到的客户端强制的边界是证据，而本模块发明的边界不是 |
| 联系电话 | 大陆手机号（`1[3-9]…` / `15[036789]…` / `18[89]…`，11 位） | 参考实现自己的正则，逐字复现；在**任何请求存在之前**生效 |
| 场馆 / 项目号 / 账号 id | 1–10 位纯数字 | 与只读半的解析上界一致，使读回来的值一定写得回去 |

`SportsPhone` 与 `SportsCaptchaCode` 都包 `Zeroizing<String>`，`Debug` 只打 `digits: N` / `chars: N`，`expose()` 是 `pub(crate)`；`SportsWritePlan` 的 `Debug` 只打 `operation` / `method` / `path` / query **段数** / `field_count`，**不**打 query 本身（联系电话路由的 query 里装着账号 id 与号码）。

**SDK / FFI / Dart**

- 引擎：新增 `sports_write`（17 常量 / 6 struct / 7 enum / 3 fn）与 `#[cfg(test)] mod sports_write_tests`；`api/runtime_sports_write.rs` 新增 `load_sports_captcha` / `book_slot` / `cancel_reservation` / `ensure_sports_write_session` / `write_adapter` / `finish_sports_write`；runtime 新增 `load_sports_captcha` / `book_sports_slot` / `cancel_sports_reservation` 三个公开方法、`sports_slot_selectors` / `sports_slot_owner` / `sports_slot_at` / `sports_reservation_selectors` / `sports_reservation_owner` / `sports_reservation_at` / `sports_confirmed_phone` 七个状态字段，`ConfirmedSportsSlot` 一个私有 struct，`confirm_sports_slots` / `confirm_sports_reservations` / `selected_confirmed_slot` / `clear_sports_failure_code` 与 `invalidate_sports_session` 的扩展。`reference_test_support::Reply::body` 从 `String` 改成 `Vec<u8>`（新增 `Reply::bytes`），因为验证码路由答的是**图片字节**，fixture 必须能原样送出这些字节而不是一次有损的再编码；由此 22 个既有 fixture 断言的 `String::new()` 改成 `Vec::new()`。
- SDK：`SportsClient::{captcha, make_order, unsubscribe}`（薄包装，与 `ElectricityClient` 同构），`sports` 模块 re-export `SportsCaptcha` 与 `SportsWriteOutcome`；`public_api.rs` 新增 `compile_sports_api` 的写半与一条新断言。
- FFI：`ClientHandle::{sports_captcha, sports_make_order, sports_cancel_reservation}`、`SportsCaptchaDto`、`pub(crate)` 的 `SportsSlotRef` / `SportsReservationRef`（私有构造、`Debug` 只打 `selector_len`）与两个 `replace_sports_*_references`；`invalidate_auth_bound_references` 一并清空这两张表。FRB 2.13.0 重新生成，生成物未手工编辑。
- Dart：`lib/src/sports.dart` 新增 `SportsSlotReference` / `SportsReservationReference`（`const X._(this._id)` 私有构造）、`SportsCaptcha`（`Uint8List.fromList` 防御性拷贝）与三个方法；`SportsResource.bookable` / `SportsReservationRecord.withdrawable` 承载句柄；`lib/sports.dart` 入口与 `test/public_entrypoints_test.dart` 同步。

**验证**

引擎定向：`cargo test -p tsinghua_kit_engine --lib sports` **27 项通过 / 0 失败**（`sports_tests` 16 项只读 + `sports_write_tests` 9 项写 + `api::runtime::sports_write_runtime` 2 项；§61 的 16 项不变）。写半 9 项覆盖：下单体恰好是观测到的 13 个字段且 booking hash 只出现在 `allFieldTime` 里、账号 id 不进下单体、只有场馆自己的 `预定成功` 算接受而**另一句可读的 `msg` 是拒绝**（且拒绝措辞不出现在 `Debug` 里）、HTML/空体/无 `msg` 的对象一律未确认而**不是**拒绝、未跟随的重定向判未确认且**只发一次**、401 与登录重定向在任何体被读取之前判会话失效、验证码是图片否则是错误（登录页冒充 `image/png` 与真图冒充 `text/html` 两种都拒）、退订体只有 `bookId` 且可读但非肯定的回答是未确认、联系电话路由体为空且 query 里带账号 id、`Debug` 不出现验证码 / `res_hash` / 手机号 / 账号 id、每个调用方值（空与超长）、成本（空/带空格/科学计数/超 42）、场馆与日期（`2024-09-31`、`2024-9-20`）、hash 形状、账号 id 非数字都在**任何请求存在之前**被拒、支付链的三条路径都无法被任何计划指向。runtime 2 项断言 selector 只对**产生它的那次读与那个账号**有效（别人的账号、未知 selector、超过 300 秒、没有 owner 四种都解析为 `None`），以及单笔上界确实是观察到的那个常量。

桥接层 `cargo test -p tsinghua_kit_ffi --lib` **55 项通过**（本轮 2 项定向：`Debug` 不出现 booking token、账号手机号、私有 selector 与验证码字节，而 `SportsCaptchaDto` 只打 `content_type` 与 `byte_len`；被拒参数零请求且 `auth_status` 仍是 `SignedOut`）。SDK `cargo test -p tsinghua_kit --test public_api` **24 项通过**（新增一项：无会话时 `captcha` / `make_order` / `unsubscribe` 都以 `Service::Sports` + `SessionRequired` 返回、**不是** `OutcomeUnconfirmed`，且账号仍是 `SignedOut`——一个根本没派发的调用没有未知效果）。`cargo check --workspace --all-targets` 退出 0；`cargo fmt --all -- --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 干净；`flutter analyze lib test` 无问题；`flutter test test/public_entrypoints_test.dart` 通过并覆盖 `SportsCaptcha` 与两个句柄类型（两者都无可公开构造，因此只有场馆的读能铸出一个句柄）。

**同过滤器下的既有失败**：`library` 过滤器仍为既有的 4 项失败，`tsinghua_kit_ffi` 的 `classroom_contract` 集成测试 3 项 `MissingDateHeaders` 失败，引擎 rustdoc `-D warnings` 仍有 4 条既有私有链接错误（`assessment_read`/`invoice_read`/`library_room_read`/`reserves_read`），整套 `--lib` 引擎运行仍会因既有栈溢出中断（`backend_repair_learn_fresh_announcement_cache_skips_live_handoff`）——均为 HEAD 既有、与本轮无关（见 §64/§67/§68/§69 的归因），本轮**未修**。

**baseline**

`docs/api-surface-baseline.json` 已按本节源码与重新渲染的 Rustdoc 刷新到第十二版：`source_revision` 前进到 `af1e96e`（§69 那一次提交；本轮改动尚未提交）；`root_public_modules` 70 → **71**（新增 `sports_write`）；根 `pub use` 57 → **58**；`rendered_crate_root_item_counts` struct 358 → **364**、enum 178 → **184**、fn 66 → **69**、constant 83 → **100**（本模块 26 个公开项在 crate 根渲染为 6 struct / 6 enum / 17 constant / 3 fn；另有 §61 已计过的只读半不变），`trait` / `type` 不变；runtime `public_methods` 108 → **111**（新增 `load_sports_captcha` / `book_sports_slot` / `cancel_sports_reservation`，行号 8379 / 8403 / 8421，插在 `apply_campus_card_write` 与 `cancel_library_booking` 之间——**不是**列表末尾，§69 那一次把它记在末尾是错的；本版按当前源码**逐条重新锚定全部 111 条**并同时修正了 `public_free_functions` 6 条的行号：`impl` 之前的插入使前 52 条方法（含被错排在末尾的 `apply_dorm_password_reset`）下移 77 行，三个新方法之后的 51 条再下移 59 行，`load_sports_records_result` 再下移 7 行、其后的 4 条 `reserves` / `library_room` 方法再下移 7 行），`direct_state_field_count` 128 → **135**（新增七个），`line_count` 29232 → **29464**（与 `git diff --numstat` 的 `245 13` 一致：净 +232）；runtime `dto_structs` 84（本轮无新 runtime DTO，`SportsCaptchaDto` 定义在 FFI crate）。

**未验证**：三条写路由与验证码读的线上可用性**未验证**，需要另行真实只读验收。本轮**未对任何真实账号发起任何请求**，**未发起下单、退订、支付或手机号更新**，也**未请求过任何真实验证码图片**。按 §59 起的约定，下单与退订**一律不进入只读验收**：它们的"结果是否未确认"只能作为判定语义来验，不能作为线上证据。在此之前不得用 fixture 或空结果冒充线上证据。宿舍卫生分的结论（§58）不变。

## 71. 2026-09-30 只读验收脚本补齐全部可读域

§53–§70 把十几个只读域逐一实现进引擎、SDK、桥接与 Dart，但终端只读验收脚本 `api::runtime::cli_validation::CHECKS` 只覆盖到 §52 那一批：54 项、12 个服务。也就是说，**新域在代码里能读，在验收脚本里却选不到**——`--case` 会以 `unknown_case_selection` 拒绝，`--plan` 根本不会列出它们。本节把这件事补齐：凡是"只有读、没有写"的能力，脚本里都必须有一个可选中、可判定、可留痕的用例。

**新增 20 项（54 → 74 项，12 → 24 个服务键）**

| 用例 | 服务键 | 依赖 | 判定 |
| --- | --- | --- | --- |
| `program_completion` | `program` | `info_session` | `report.course_sets.len()` |
| `physical_exam_result` | `physical_exam` | `info_session` | 有记录的项目数（`items.reported().len()`） |
| `assessment_list` | `assessment` | `info_session` | `items.len()` |
| `assessment_form` | `assessment` | `assessment_list` | `field_count` |
| `invoice_list` | `invoice` | `info_session` | `records.len()` |
| `invoice_document` | `invoice` | `invoice_list` | `bytes.len()` |
| `bank_payment_ledger` | `bank` | `info_session` | `receipt_count` |
| `bank_foundation_ledger` | `bank` | `info_session` | `receipt_count` |
| `graduate_income` | `graduate_income` | `info_session` | `records.len()` |
| `course_score` | `course_score` | `info_session` + `learn_courses` | 固定 1（一次查询一个课程号） |
| `sports_resources` | `sports` | `info_session` | `resources.data.len()` |
| `sports_records` | `sports` | `info_session` | `records.len()` |
| `reserves_search` | `reserves` | `info_session` | `books.len()` |
| `reserves_detail` | `reserves` | `reserves_search` | `book.chapters.len()` |
| `library_room_catalog` | `library_room` | `info_session` | `room_count` |
| `library_room_records` | `library_room` | `info_session` | `records.len()` |
| `library_reservations` | `library` | `library_session` | `reservations.len()` |
| `laundry_buildings` | `laundry` | 无 | 三家厂商的楼栋数之和 |
| `laundry_rooms` | `laundry` | `laundry_buildings` | `rooms.len()` |
| `water_user` | `water` | 无 | 不读，见下 |

`ServiceId` 一个都没加（仍然是 7 个变体），`service_catalog` 一条都没动：服务键只用于调度优先级、`_session` 启发式与 tracing span。`library_reservations` 不是新域，而是一条**既有却从未被验收的读**（§64 的座位预约记录），本轮一并补上。

**判定口径：空结果不等于通过**

新用例统一沿用既有形状：先调用运行时读、再 `validation_scope::require_live_validation_result(&result.source, &result.status)?`。除此之外每条还加了自己的形状检查，因为"服务给了空列表"和"读根本没成立"必须在报告里是两件事：

- `physical_exam_result` 用**有记录的项目数**而不是全零项目数：`no_result` 为假而一个项目都没有，说明解析或字段名对不上，报错而不是报 0。
- `assessment_form` 用 `field_count` 并要求引用往返相等；列表里**没有一条未评价的问卷**时跳过，而不是打开一份已经填过的表（参考实现的入口也是未填写的问卷）。
- `invoice_document` / `reserves_detail` 都要求字节或章节非空——文档读回来是空的就是失败。
- `bank_*_ledger` 用 `receipt_count` 而不是 `months.len()`：月份可以是空段。
- `sports_records` / `library_room_records` / `library_room_catalog` / `graduate_income` / `program_completion` 都要求 `error.is_none()`。
- `laundry_buildings` 要求**至少一家厂商**回答了：三家全挂时把第一家的诊断码作为失败抛出，不允许把"什么都没读到"当成空目录。
- `laundry_rooms` 在厂商自己的 `failed_categories` 非空时判失败（`laundry_rooms_incomplete`），因为那是厂商明说"这一半没读全"。

**样本从哪来**

- 课程成绩：课程号取自本轮 `learn_courses` 已经读到的**课程号字段**（`LearnValidationEvidence::validation_course_ids()`，逐条先用 `course_score::course_id_for_request` 过滤形状），学号仍由 Rust 从已绑定账号内部派生。两者都不是调用方输入，脚本也没有任何新交互提示。
- 日期窗口：研究生收入用 `today - 364 天` 到 `today`（`%Y%m%d`）；研读间记录用 `today - 29 天` 到 `today`（`%Y-%m-%d`，落在模块自己的 31 天上界内）；体育场馆用今天。
- 体育场馆的场地号：`SPORTS_VENUE_SAMPLES` 是一张**固定表**，与参考客户端自己的 `sportsIdInfoList` 同源。服务没有任何"列出场馆"的读，所以唯一的诚实来源就是这个公开常量；因此这里不假装它是服务发现的。写法是**逐个试**：某个场地三次读失败就换下一个，只有全都失败才报最后一个失败，而"场地存在但没有可约时段"本身算通过。
- 馆藏检索关键词：固定 ASCII 词 `physics`，而不是某个人的书名。空匹配是服务自己给出的答案。
- 订水 `water_user`：查询要一个**送水编号**，那是只有本人知道的调用方输入。终端可接受的提示标签是**封闭集合**（`env_auth.rs` 只认三个标签），新增提示会破坏 `--credentials-env` 路径，所以这一项**不读**，显式记为 `water_delivery_number_unavailable`（未验证），而不是猜一个编号或拿空结果冒充。

**跳过原因也进白名单**

验收脚本里**每一个** `Outcome::Skipped` 的原因码现在都在 `telemetry_labels::REASONS` 里：本轮新域的 `no_assessment_selector` / `no_invoice_selector` / `no_reserves_selector` / `no_sports_venue_selector` / `no_course_number_selector` / `no_laundry_building_selector` / `water_delivery_number_unavailable`，以及既有遗漏的 `no_subscription_selector` / `no_phase_selector` / `no_file_selector` / `no_homework_selector` / `homework_sample_limit`。不登记也不会失败——`labels::allowed` 只是审计日志把该字段抹掉、`--status` 印出空原因——但登记之后原因码本身留在日志里，不必回头猜。

新增测试 `backend_repair_read_only_acceptance_skip_reasons_survive_the_audit_log` 直接扫 `cli_validation.rs` 源码里所有 `Outcome::Skipped(...)` 的字符串字面量，逐个要求 `telemetry::diagnostic_reason(code) == code`。也就是说以后再加跳过分支而忘了登记，测试会**立刻**失败，而不是等到某次线上验收发现原因栏是空的。这条断言只在既有白名单之外**新增**条目，不改任何既有映射。

仍然未登记的是各域自己的**失败**诊断码（`program_*`、`sports_*`、`assessment_*`、`bank_*`、`invoice_*`、`reserves_*`、`library_room_*`、`physical_exam_*`、`water_*` 等，全仓库 381 个，其中 `info_*` 50 个、`learn_*` 46 个也是既有缺口）。它们让失败在报告里落成 `response_unconfirmed` / `network` 这类粗粒度值。这是一整批语义映射工作（每个码都要确认它想表达什么），本轮**未改**，留待专门一轮；本轮只保证"跳过原因"这一侧的日志可读性。

**App 侧台账同步**

`live_validation.rs::select_cases` 自己的 `KNOWN`（App 的 `backend-live-results.json` 台账选择器，与 CLI 的选择器是两份）同步加入这 20 个 id，否则 App 侧 `backend-live-validation` 模式下这些项既选不了也记不了。

**验证**

引擎定向：`cargo test -p tsinghua_kit_engine --lib read_only_acceptance` **8 项通过**（新文件 `api/cli_read_only_acceptance_tests.rs`，挂在 `cli_validation_tests` 下以复用 fixture）：

- 20 个 id 都在 `CHECKS` 里可选中，且 `CHECKS` 无重复 id；
- 验收脚本能报出的**每一个跳过原因**都是固定原因码（`diagnostic_reason` 原样回读），加新分支不登记会当场失败；
- 依赖闭合只拉需要的会话：`assessment_form` 只要 info 链、`invoice_document`/`reserves_detail` 走各自的列表依赖、`course_score` 同时要 learn 链与 info、`library_reservations` 要 library 链、`laundry_rooms`/`water_user` **不**拉任何校园会话（两家第三方读本来就不需要账号）；
- 12 个受校园会话管辖的域在**没有会话**时全部报错，且 fixture 上**零请求**；
- `course_score` 没有课程证据时报 `course_evidence_unavailable`，零请求（不会凭空造一个课程号）；
- 四个"句柄缺失"用例各自以正确的原因退成 `Unverified`，零请求；
- `laundry_rooms` 的断言对**可达与不可达两种结果都成立**（厂商是公网服务，测试机可能真的连得上）：通过必须至少一间房，跳过只能是 `no_laundry_building_selector`，失败必须带厂商自己的原因码；
- `usereg_session` 未选择时是 `optional_login_not_selected`，选择了但没有登录页时是失败而不是空账户。

`cargo check --workspace --all-targets` 退出 0；`cargo fmt --all -- --check` 干净；严格 `RUSTDOCFLAGS="-D warnings" cargo doc -p tsinghua_kit` 干净。**没有真实账号请求**：本轮全部判定都来自 fixture，任何一项的真实可用性仍为未验证。

**同过滤器下的既有失败**：`--lib cli_` 过滤器下 **93 通过 / 6 失败**，`git stash push -u` 对照 HEAD 为 **85 通过 / 6 失败**——失败集合逐条相同，8 项新增通过，本轮没有引入新的失败：

- `network_scope_tests::backend_repair_off_campus_is_explicit_and_never_exempts_webvpn_services`
- `coverage_tests::backend_repair_coverage_scheduler_preserves_environment_and_login_gaps`
- `tests::backend_repair_terminal_interrupted_case_keeps_actual_dispatched_request_count`
- `timing_tests::backend_repair_perf_actual_scheduler_admits_multiple_ready_cases_without_fake_http_parallelism`
- `timing_tests::backend_repair_perf_scheduler_propagates_failed_dependencies_and_separates_user_time`
- `tests::backend_repair_graduate_exams_cli_checks_live_results_instead_of_skipping`

它们需要真实 loopback HTTP（sandbox 内被拦）或更强的隔离；本轮**未修**，归因由 stash 对照给出而不是推断。仓库自带的 `python3 tools/backend_regression.py` 才是这些项的既定运行入口。

**baseline**

本轮**不移动** `docs/api-surface-baseline.json` 的任何计数：改动只在 `cli_validation.rs`（验收脚本用例表、`Evidence` 四个字段、两张模块常量表）、`runtime_validation_scope.rs` 的一个 `pub(super)` 访问器、`live_validation.rs` 的选择器白名单、`telemetry_labels.rs` 的 `REASONS` 新增条目与新增测试文件里，`runtime.rs` 一行未动（`git diff HEAD -- .../runtime.rs` 为空），模块列表、根 `pub use` 计数、渲染根项计数、runtime 方法与字段计数、`line_count` 全部不变。按 baseline 自己的方法论（"只统计公开模块/方法/DTO 声明与渲染根项"），验收脚本的私有用例表与 `REASONS` 的固定词表都不在其统计口径内；本版只把 `source_revision` 前进到 `d8db976`（本节改动尚未提交）。

**未验证**：本节新增的 20 项线上可用性**全部未验证**，需要另行真实只读验收。本轮**未对任何真实账号发起任何请求**，也**未发起任何写操作**。按 §59 起的约定，写操作（体育场馆下单与退订、校园卡挂失/解挂/改密/改限额/圈存、宿舍口令替换、图书馆预约与取消、评估提交、新闻订阅与收藏）**一律不进入只读验收**，因此本节只补读半。宿舍卫生分的结论（§58）不变：它不进入脚本，因为该 selector 的第二跳返回的是图片。
