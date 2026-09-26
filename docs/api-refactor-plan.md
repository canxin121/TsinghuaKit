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
