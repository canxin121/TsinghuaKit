# Flutter 旧 API 到 TsinghuaKit 的迁移矩阵

状态：迁移规划与 facade 分批实施中。App 仍使用 `v0.1.1` 和自己的 FRB 生成绑定；公共库工作树当前为 `0.2.0-alpha.1`。清单来自 App 的 `CampusRuntimeGateway` 及其 companion gateway 接口，合计 72 个旧异步入口，不包含页面状态、缓存展示器和纯模型校验器。此文档不表示线上服务已验证。

迁移的主约束是，一个 Flutter `TsinghuaKitClient` 只持有一个 Rust `Client`。身份、网络自助和业务服务共享该 Client 的 Runtime/transport；门户本机连接资料留在 `client.network.profiles`，不增加 Auth slot。引用型选择必须由前一次 SDK 读取产生，并由 Rust 绑定 Client 与目录代次；Flutter 不传 URL、学校 selector、数组下标或自由拼装的业务 ID。

| App 旧接口 | 新边界与 Rust SDK 对应 | Flutter/FRB 状态 | 迁移说明 |
| --- | --- | --- | --- |
| `status`, `login`, `sendSecondFactorCode`, `completeSecondFactor`, `logout` | `client.auth().identity()` 与 `client.auth().status()` | 登录、当前二次认证交互查询、验证码、双账号状态、分域退出和全退均已接；旧 App 未迁 | 拆成 `Identity` 与 `SelfService` 两个独立 Auth 域；统一认证派生的服务证明仍属于同一个 Identity。未知认证方式保持 unknown 并在提交前拒绝。 |
| `inferAcademicStage` | `TsinghuaKit.suggestLoginStage(username)` 作为本地 Identity 登录提示；登录仍显式传入 `LoginStage` | Rust/Flutter/FRB 已桥接；旧 App 尚未迁移 | 推断不创建 Client、不触发请求或认证；无匹配规则和未来未知阶段返回 `null`。迁移时保留学号变更后重算与用户选择优先语义，选择不写入持久会话。 |
| `reconcileSession`, `hydrateLocalSession` | `client.auth.identity.revalidateRestoredSession()`；本机网络资料仍由 `client.network.profiles` 独立读取 | Identity 快照恢复与显式重新验证已桥接；SelfService 持久状态及 App hydration 尚未迁 | 默认 Auth 仍为内存；显式目录快照在首次成功 Identity 检查点只写一次当时的共享 Cookie jar，后续业务/SelfService Cookie 不刷新磁盘快照，但保存时已存在的 Cookie 仍可能在其中，因此不是 Cookie 分区。恢复状态为 `RestoredUnverified`，显式重新验证才发只读请求；schema 1 快照不迁移。目录密钥与密文同处，OS Keychain 尚未接入；THYou App 整体切换仍被阻断。 |
| `serviceCatalog` | App 自己的显示目录与 capability registry | 不进入 TsinghuaKit facade | 图标、本地化 key、首页偏好和服务卡片属于 THYou UI。Rust capability 证明只由实际 SDK 服务方法表达，不能作为 UI 展示对象。 |
| `establishServiceSession` | 不单独公开；由首次实际业务读取按需建立 handoff | 迁移时删除旧调用 | 不允许页面先预建会话再读取；共享服务 handoff 由同一 Rust Client 管理，避免重复认证链。 |
| `loadOverview`, `peekOverview`, `refreshOverview` | App 高层组合多个类型化服务结果 | 未桥接 | 不是新的底层认证/HTTP API。迁移后 Overview repository 可在同一 Client 上并行查询独立服务，保留各分区错误和 source metadata，不把请求失败压成空结果。 |
| `loadGrades`, `loadSemesterSchedule`, `loadExams` | Rust `client.registrar().grades()`, `semester_schedule()`, `exams()`；Flutter `client.registrar.grades()`, `semesterSchedule()`, `exams()` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | 统一用带来源、新鲜度和覆盖证明的 `ReadResult`；删除旧的普通/`Result` 重复入口。考试日期不补造缺失年份，研究生考试标签按原始服务数据呈现。 |
| `loadThosPending`, `loadThosServices`, `loadThosTaskList`, `loadThosPhaseSteps` | `client.service_hall().pending/services/tasks/phase_details` | 待办、目录、四种任务视图、阶段详情均已桥接到同一 handle | 完整分页、首页单独计数、退回合并和 freshness 保留。Dart 不接收 selector；阶段详情引用 Client-bound，不能用 task ID 或 list index 替代。 |
| `loadInfoNews`, `searchInfoNews`, `loadInfoNewsDetail`, `loadInfoNewsDetailResult`, `loadInfoNewsCatalog`, `loadInfoNewsSubscriptions`, `loadInfoNewsFavorites`, `loadInfoNewsSubscriptionPage` | Rust `client.news().catalog/subscriptions/favorites/articles/article/subscription_articles`；Flutter `client.news.catalog/articles/search/article/favorites/subscriptions/subscriptionArticles` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | source/channel 与 subscription 只能从本 Client 最近一次目录/订阅读取取得不透明引用。详情必须使用当前 news page 产生的 `NewsArticleReference`，不接收任意 `articleId`。普通 detail 与 detail result 收敛为一个保留 provenance 的结果。 |
| `loadLearnCourses`, `loadLearnCourseList`, `loadLearnAnnouncements`, `loadLearnAnnouncementList` | `client.learn().courses/announcements`；Flutter `client.learn.courses()/announcements()` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | 公共结果保留 Runtime 已证明的新鲜/陈旧缓存语义；兼容 list DTO 不作为另一套逻辑来源。Dart `LearnCourseReference` 只由当前 Client 的课程目录创建。 |
| `loadLearnHomework`, `loadLearnHomeworkDetail` | `client.learn().homework/homework_detail`；Flutter `client.learn.homework()/homeworkDetail()` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | 使用当前课程目录生成的 `LearnCourseReference` 和最新作业列表产生的 `LearnHomeworkReference`；不把自由课程 ID 或作业 ID 暴露为详情调用参数。只读详情不包含提交。UTC 时间转换为 UTC `DateTime`。 |
| `loadLearnFiles`, `loadLearnFileCategories`, `downloadLearnFile` | `client.learn().files/file_categories/save_file`；Flutter `client.learn.files()/fileCategories()/saveFile()` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | `LearnCourseFileReference` 绑定 Client/最新文件目录；保存需用户明确选定目标路径，拒绝覆盖现有文件。当前 API 不支持内嵌预览或返回任意文件字节；不得用 Dart 直接请求学校 URL。 |
| `loadLearnDiscussions` | `client.learn().discussions`；Flutter `client.learn.discussions()` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | 保留服务端最多 200 条限制与 partial coverage；时间标签按服务显示值呈现，不臆造时区。帖子详情/发帖不在当前只读 SDK 范围。 |
| `loadLearnTermCalendar`, `loadSchoolCalendar` | Rust `client.calendar().learn_terms/school_calendar`；Flutter `client.calendar.learnTerms()/schoolCalendarImage()` | Rust 与 Flutter/FRB 已接入同一个 ClientHandle；旧 App 未迁 | 学校校历语言、学期和可选年份走类型化 query；越界年份和未知 selector 在请求前拒绝，图片响应及学期时间线由 Rust 验证。Learn 学期读取仍要求 Identity 会话。 |
| `loadLibraryAreaTree`, `loadLibraryAreaTreeResult`, `loadLibraryFloors`, `loadLibrarySectionsForDay` | `client.library().directory/floors/sections` | Rust 与 Flutter/FRB facade 已接入；App 尚未迁移 | 保留场馆、楼层、分区的不可伪造引用；旧自由 BigInt ID 要在 App 迁移时消失。 |
| `loadLibraryDaySegments`, `loadLibraryDaySegmentsResult`, `loadLibraryDaySegmentsForDayResult`, `loadLibrarySeats`, `loadLibrarySocketStatus` | `client.library().time_windows/seats/sockets` | Rust 与 Flutter/FRB facade 已接入；App 尚未迁移 | 日期、场馆/楼层/区域、开放时段与座位选择逐层绑定；缓存来源/新鲜度和列表覆盖保留。 |
| `loadClassroomBuildings`, `loadClassroomBuildingsResult`, `loadClassroomState` | `client.classrooms().buildings/availability` | Rust 与 Flutter/FRB facade 已接入；App 尚未迁移 | 用当前楼栋目录的 `BuildingRef`，不以楼栋下标或自由 ID 读取；结果保留来源和学周选择。 |
| `loadElectricityRemainder`, `loadElectricityRemainderResult`, `loadElectricityPaymentHistory`, `loadElectricityPaymentHistoryResult` | `client.electricity().remainder/payment_history` | Rust 与 Flutter/FRB facade 已接入；App 尚未迁移 | 普通与 envelope 重复入口合并成 `ReadResult`；宿舍位置/账号绑定错误维持明确错误。 |
| `loadCampusCardAccount`, `loadCampusCardAccountResult`, `loadCampusCardTransactions`, `loadCampusCardTransactionsResult` | `client.campus_card().account/transactions` | Rust 与 Flutter/FRB facade 已接入；App 尚未迁移 | 日期窗口最多 31 天、完整分页后返回、来源信息保留。卡片目标密码是绑定 Identity handoff 的一次性交互，不是第三个账号。 |
| `startUseregLogin`, `refreshUseregCaptcha`, `completeUseregLogin`, `useregLoginPhase`, `cancelUseregLogin` | `client.auth().self_service()` | Captcha 登录、刷新、提交、取消、登录阶段和分域退出均已桥接；App 尚未迁移 | 独立网络自助账号使用自己的 username/password 和图片验证码。登录阶段是本地固定枚举；刷新 captcha 不重提密码；不得自动重登、自动刷新后提交或把 Identity 账号复用到该域。 |
| `loadUseregAccount`, `loadUseregBalance`, `loadUseregDevices`, `disconnectUseregDevice` | `client.self_service().account/usage/online_devices/disconnect_device` | Rust 与 Flutter/FRB 已接入，运行在与 Auth 共用的 ClientHandle | 账号、设备、用量均返回 `ReadResult`；断开使用当前设备目录的 Client-bound 不透明引用，且在发送一次性断开前消费引用；业务结果要求 SelfService 自己的 Auth slot。 |
| `loadTunetStatus`, `loginTunet`, `disconnectTunet` | `client.network()` 的本机网络观察/显式操作 | 只读 Portal 登记观察和本机资料 Rust API 已有；操作及 Flutter 尚未桥接 | 不是 Auth slot，也没有服务端 session token 可持久维护。用户名/口令属于 Portal 本机连接资料，可选择保存并显式填入；Tsinghua Secure 的系统 EAP 配置由 OS adapter 处理。Portal、物理网卡状态和 Tsinghua Secure EAP 必须分别报告，不能用 Portal 已登记推断设备在线。 |

## 迁移门禁与顺序

1. Auth/网络资料、service-hall、SelfService、Registrar/Calendar、INFO、Learn、Library、Classroom、CampusCard 和 Electricity 均已有首批 Rust/Dart facade。FRB 绑定只由 codegen 产出；后续扩展仍沿用同一 Rust ClientHandle。
2. 设计 Client 级 Auth 恢复：两个账号槽分别绑定，凭证材料通过宿主 Keychain/secret-store adapter 管理；会话恢复失败必须给出类型化状态，不触发隐式重复登录。不得把 Portal/EAP profile、Auth 会话与业务缓存合并存储。
3. 补齐剩余的 Auth 恢复、网络资料安全存储适配和 Portal/OS 网络能力；每一域都在同一 `ClientHandle` 上增加 DTO/方法，不建立平行 runtime。Profile 内容不能和两个 Auth 会话混合持久化。
4. 对照 App 实际引用点迁移 repositories/controllers：使用 package 的领域类型和稳定错误；App 可以做 UI 排版及调用编排，不复制 Rust selector、HTTP、解析、缓存策略、账号绑定或错误分类。
5. 删除 App 的 `lib/src/rust` 生成目录和旧 `FrbCampusRuntimeGateway`，再更新 `pubspec.yaml` 至 public TsinghuaKit tag。进入该步骤前必须有全量调用映射、相同 Client 生命周期、session recovery 方案、平台构建与必要业务验收证据。

## 暂未纳入迁移的 App 职责

`serviceCatalog` 的图标/本地化/偏好、首页 widget 排序、隐藏区块、页面路由、窗口状态、日期显示格式等继续留在 App。`overview` 是这些领域读取的聚合视图，不产生额外后台 Client。TUNet 注册态只是一项网络证据；本机 EAP 是否连接、物理接口 IP、代理/隧道路由由系统层分别观察，不能映射到 Auth 会话状态。
