# TsinghuaKit 双账号 Auth 与本地校园网连接设计

状态：设计与分阶段实现并行。更新日期：2026-09-26。对应 [公共 API 重构方案](api-refactor-plan.md)。当前存储决定：SDK 自己管理应用私有目录下的文件，不调用 Keychain、Keystore、Flutter Secure Storage 或其它系统凭据存储；下文早期安全存储提案以本节“文件持久化现状”为准。

公共 SDK crate 和内部 engine 已拆开；Rust `Client` 目前支持一个共享 Runtime、Identity 登录/二次认证、独立 SelfService 验证码登录、双账号状态及多项校园服务。Auth 会话默认只驻留内存；保存凭据、Identity Cookie 快照和网络资料均需显式选择。SDK 将选择保存的数据分别写入应用私有目录中的加密文件，密钥与对应文件保存在同一目录；它不使用系统凭据存储，也不防范能读取应用目录的同一用户进程。Identity 快照进程重启后只恢复为 `RestoredUnverified`，必须显式调用 `identity.revalidateRestoredSession()`。Identity 与 SelfService 凭据按 Auth 域隔离；保存的 SelfService 密码只启动新的验证码流程。SelfService 跨进程会话恢复尚未实现。SDK 仅在首个成功的 Identity 持久化边界保存当时的共享 Cookie jar，不构成严格账号 Cookie 分区。TUNet Portal 和 Tsinghua Secure/EAP 仅属于本机网络资料和操作，不是第三个 Auth 账号。当前 Flutter facade 已覆盖双账号 Auth、会话恢复查询、网络资料 CRUD/显式填写和多项校园服务；THYou App 生产 gateway、认证 hydration 和页面数据流尚未迁至新 Client。下面未标为“已实现”的接口图均为目标设计。

本设计采用用户明确的产品模型：Auth 可能同时有统一身份、网络自助两个账号；校园网连接是本地网络操作，可保存资料和自动填写，不作为需要持久维护的第三个账号会话。

## 1. 先固定三个对象的区别

| 对象 | 统一身份账号 | 网络自助账号（USEREG） | 校园网连接资料 |
| --- | --- | --- | --- |
| 用途 | 身份认证及相关校园业务 | 自助账户、用量/余额、设备管理 | 给本机校园网连接提供输入 |
| 是否属于 Auth | 是，`Identity` | 是，`SelfService` | 否 |
| username 是否必须与另一项相同 | 否 | 否 | 否，允许独立内容 |
| 是否管理在线业务会话 | 管理身份与派生服务证明 | 管理独立业务会话及账号证明 | 不管理可恢复/续期的第三类账号会话 |
| 允许保存什么 | 账号资料、受控会话状态、显式选择保存的凭据/设备信息 | 自己的账号资料、适用的受控会话状态与可选凭据 | 资料名称、账号输入、连接方式、可选密码、默认选择 |
| 重启后如何处理 | 加载本地资料，按策略验证/恢复会话 | 加载自己的资料；满足通道条件后验证/恢复 | 仅在宿主显式 opt-in 持久化时恢复填写资料；网络状态重新读取 |
| 退出/删除的行为 | 退出或遗忘该账号 | 退出或遗忘该账号 | 删除资料或显式断开连接，两个操作分开 |

一期一个 Client 包含两个可选账号槽，每个槽至多一个活动账号。多个账号同时在线的调度器不在本轮范围内。连接资料的 schema 可以容纳多条，首版 UI 可以只提供一个默认资料。

WebVPN、Learn、教务、校园卡等仍然可以有业务 Cookie 和服务证明，但这些不构成更多的用户账号槽。校方返回的业务凭据和用户输入账号属于不同层次。

校园卡 SSO 页面可能要求一次目标服务密码。它是当前 Identity 账号针对校园卡 handoff 的短期交互输入，不会创建 `CampusCard` Auth 账号，也不会进入账号恢复链；请求不明确时不重放。

同一个 username 出现在两个 Auth 域或连接资料中，也必须视为不同用途。密码相同与否不由 SDK 推测，不联动覆盖。

## 2. 当前实现说明了哪些需要改动的边界

- `UseregPendingLogin` 已同时持有 USEREG 输入凭据和 `identity_owner`。登录完成后用 USEREG 输入账号做账户确认，而非强制等于统一身份账号。这是应保留的双主体基础。
- USEREG 登录目前要求统一身份与 WebVPN 前置证明，且使用共享的受控 transport。独立业务账号不意味着已具备无前置条件的直连路径。
- 当前 Runtime 的 TUNet 登录已不再调用 `begin_authentication` 或 `mark_authenticated`。请求复用 Runtime 的 `CampusHttpTransport`；成功后仅把账号、IPv4 和 client 放入本进程的 `TunetConnectionTarget`，供同进程的一次显式断开使用。
- `load_tunet_status` 无需任何 Auth 槽或 TUNet session 即可只读检查当前 IPv4；如果地址变化，会废止旧断开目标并为新地址重新观测。Tsinghua Secure 仍由操作系统管理，不能从本机有 IP 推断其 Wi-Fi 身份。
- TUNet 已从 `protocol::ServiceId` 与 `SessionRegistry` 移除；门户响应以 `Service::Network` 标识，不会进入 Auth 恢复或服务快照。兼容目录把 TUNet 的来源标成 `local_network_context`，并只在进程内断开目标存在时公布断开能力。
- 兼容层旧 `CampusRuntime::logout()` 仍表示全局重置；新 SDK 已将 `Identity`、`SelfService` 与 `logout_all()` 分开。Identity 注销会一并废止其派生业务证明；如果 SelfService 账号仍被选中，它保留为 `Expired`，明确表示 B 的选择还在、共享 Identity/WebVPN 通道已经关闭。SelfService 单域注销只清除 B 的会话、挑战和设备目标，不影响 A 或共享 transport。
- 当前凭据记录带学段字段，明显面向统一身份。添加 USEREG 和连接资料时，需要新用途命名空间，不能按 username 直接复用现有记录。

证据：[当前 Runtime 中的认证与网络实现](../rust/crates/tsinghua-kit-engine/src/api/runtime.rs)、[凭据存储实现](../rust/crates/tsinghua-kit-engine/src/credential_store.rs)。

以上是当前源码边界变化；真实账号登录、验证码、Portal 连接和系统 Wi-Fi 配置均未执行。Rust API 已有 Client 生命周期内的资料编辑、版本绑定的显式表单填充，以及要求同一 Client 和当前资料版本引用的显式 Portal 连接/断开操作。Auth 凭据、Identity 快照和 NetworkProfile 分别使用 SDK 管理的加密文件目录；Flutter facade 不依赖系统安全存储插件。Portal facade 目前只做了无凭据边界测试，未真实连接。App 尚无面向用户的网络资料页面，也没有操作系统 802.1X 配置能力。

## 3. 公共 API 的归属

```text
Client
├── auth()
│   ├── identity()              统一身份登录、交互、恢复、注销
│   ├── self_service()          USEREG 登录、验证码、恢复、注销
│   ├── status()                两个账号状态的汇总
│   └── logout_all()            显式退出两个账号域
├── learn()/registrar()/...     使用统一身份派生的业务证明
├── self_service()              USEREG 账号、用量、余额、设备管理
└── network()
    ├── profiles()              保存、选择、修改、删除、准备填写
    ├── status(scope)           本机/请求出口的只读网络观测
    ├── connect(request)        显式尝试连接本机校园网
    └── disconnect(target)      显式断开有充分目标证明的本机连接
```

`auth().self_service()` 与 `self_service()` 共享第二个账号槽。前者负责认证，后者负责已经获得访问条件后的业务操作。

`network()` 不以任一 Auth 账号已经登录为前提。即使两个账号槽都为空，本机也可能已经通过系统 Wi-Fi 在线；应用应能展示/检查该状态，且能在有需要时打开本机连接入口。

所有入口借用同一个 Rust Client/Runtime。账号域的区分不需要创建第二个 Runtime，更不允许绕过共享 transport 做额外登录或并发压测。

## 4. Auth 的两个账号如何建模

### 4.1 公共状态中不再有唯一全局 username

```rust
// 设计示意，字段的有效组合由 Rust 内部构造并校验。
pub enum AuthDomain {
    Identity,
    SelfService,
}

pub struct AuthStatus {
    pub identity: AccountAuthStatus,
    pub self_service: AccountAuthStatus,
}

pub struct AccountAuthStatus {
    pub account: Option<AccountSummary>,
    pub session: AccountSessionState,
    pub access: AccessStatus,
    pub persistence: AuthPersistenceSummary,
}
```

状态的四个维度分别表示：知道哪个账号、是否有当前可用的会话证明、当前访问条件如何、本地保存了哪些可选资料。保存密码、账号名可显示或缓存可读取，都不能单独推出 `Authenticated`。

`AccountSessionState` 至少区分无活动会话、正在认证、需要交互、恢复但未验证、已验证、需重新验证、明确过期和显式退出。`AccessStatus` 表达当前业务前置条件，包括缺少统一身份/WebVPN 通道、网络暂不可用等。

网络暂时不可用时可以保留尚未被明确否定的会话记录，同时将访问条件标成阻塞。访问通道所属账号/代次发生变化时，依赖它的旧在线证明则必须失效或待重证。`Ready` 必须同时满足业务证明和当前通道条件。

原先 UI 需要的“App 统一身份是否已登录”从 `identity` 槽读取；不能用汇总状态的单一布尔值代表两个账号，更不能用校园网在线代表 App 已登录。

### 4.2 业务主体与访问通道分别绑定

假设统一身份账号为 A，网络自助账号为 B：

```text
Identity(A) ──证明──> WebVPN/访问通道
                          │
                          ▼
                 USEREG 登录与读取 ──验证──> SelfService(B)
```

A 与 B 可以不同。正确验证包含两件事：请求走的是当前有效且受约束的访问通道，USEREG 返回的业务账户匹配本次输入/选定的 B。

内部上下文需要分别记录 `subject_account` 与 `access_context`：

- Learn/教务等数据属于统一身份作用域 A。
- USEREG 的余额、用量、设备和缓存属于网络自助作用域 B。
- 当前路径所需的 Identity/WebVPN 账号、Cookie 上下文和代次属于通道依赖。
- 不能把 `response.username == identity.username` 当成 USEREG 的通用验证规则。
- 也不能因为 B 独立，就删除目前实际需要的门户证明。

未来若有经过验证的 USEREG 直连路径，只更改该路由的前置依赖，业务账号仍属于 `SelfService`；本次不假设这条路径已存在。

### 4.3 两套账号代次，共用交互互斥入口

Identity、SelfService 分别维护账号代次和注销权威；共享访问通道另外有上下文代次。USEREG 请求、验证码和设备引用捕获它所依赖的全部代次。

宿主原有的 Runtime/进程写入 lease 继续生效，两套账号权威位于同一个受管上下文之下。持久化写入检查宿主 lease、对应账号域代次及必要的通道代次，不能让两个域各自启动互相覆盖 Cookie 快照的恢复写入器。

更换 B 只废止旧 B 的业务会话/挑战/设备引用。更换 A 则废止 A 的派生业务以及依赖旧通道的 USEREG 在线上下文，但保留独立保存的 B 资料。

新 SDK 的注销语义已经落地：`identity.logout()` 撤销 Identity 及所有派生服务证明；选中的 SelfService 账号保留为 `Expired`，而不是被报告为可用或悄悄删除。`self_service.logout()` 清掉 SelfService 当前账号选择、会话、验证码挑战与设备引用，同时保留 Identity 及共享 transport。`auth.logout_all()` 明确清除两个账号域。三种注销都不删除本机 NetworkProfile、不发起连接或断开操作，也不授权使用已保存密码后台重登。旧 Runtime/兼容接口的全局注销语义保持不变。

实际登录、验证码验证/提交、Cookie 变更保持串行。首版继续允许一个 Client 同时只有一个活动交互流程；其他入口返回明确的 Busy/InteractionInProgress，不自动取消已有流程，不自动重发验证码。

挑战句柄绑定认证域、账号、服务目标、有效期和依赖代次。验证码刷新使旧图片上下文失效；请求结果不明确时保留不可重放的状态。

### 4.4 两个账号分别恢复

推荐的默认恢复顺序为：加载本地账号元信息和 opt-in 配置；必要时由明确的恢复策略验证统一身份/通道；只有用户进入 USEREG 或明确请求恢复 B 时再验证其业务状态。

恢复 USEREG 遇到图片验证码等交互时返回挑战，不后台绕过或循环重登。保存过 B 的密码也不保证能静默恢复。

显式注销后写入对应域的持久注销权威，抑制后台恢复。保留下来的自动填写资料不是“下一次读取自动重新登录”的许可。

## 5. 校园网连接只保留三类状态

### 5.1 可选持久化的 NetworkProfile

连接资料包含：稳定的资料 ID、显示名称、账号输入、连接方式、可选密码记录引用、资料版本以及用户选定的默认资料。连接方式至少区分 `Portal`（srun/TUNet 门户）与 `SystemWifiEap`（Tsinghua Secure 等由操作系统管理的 802.1X 网络）。两者是同一类本机连接资料下的不同用途，不是两个新增 Auth 域。

它不包含可恢复的登录状态、登录 Cookie、srun challenge、有效在线 IP、上次在线即可信的标志。可记录最后使用时间用于展示，但不作为在线证明。Portal 密码只可用于 Portal 连接；EAP 密码只可交给明确支持的系统 Wi-Fi 适配器，绝不在两种方式之间回退或复用。

用户名可以独立于 A/B；保存连接资料不发校方请求，因此不能声称其中的密码已经验证。默认选择只是自动填写来源，不是自动提交设置。资料密码必须由宿主配置的受保护凭据存储保存，优先采用操作系统凭据存储；如果宿主选择受控文件存储，也必须遵守 Rust 侧现有的目录、加密、权限和用途绑定规则。

资料按宿主应用与操作系统用户的私有命名空间保存，不默认挂在当前统一身份账号下面。切换 A/B 不会删除或替换连接资料；资料列表也不会自动从 A/B 的密码记录中生成。TsinghuaKit 的连接资料与操作系统 Wi-Fi 配置是两个不同对象：保存或删除应用资料不声称创建、修改或删除了系统网络配置。

### 5.2 短期 ConnectionAttempt 与 ConnectionTargetRef

每次显式 `connect` 根据资料的 `NetworkAccessMethod` 进入匹配的连接器。`Portal` 在 Rust 内临时取得资料，完成目标接口选择、srun challenge、请求提交及结果证明；`SystemWifiEap` 只有在当前平台适配器声明支持且操作系统允许时才可提交系统配置/连接请求。结束后释放临时密码和 challenge，不把连接状态保存成 Auth 会话。

没有长期业务 token 不等于没有协议状态。srun 临时挑战、服务端的账号/IP 在线记录及严格的结果校验继续存在；它们不进入 Auth 的账号恢复链。

成功操作返回连接结果和必要的当前观测，并标明连接由 Portal 还是系统网络适配器处理。若支持随后断开，可提供短期 `ConnectionTargetRef`，绑定已验证的本机接口/IP、适用账号/连接方式和网络观测代次；该引用不持久化为账号会话。适配器不支持连接或系统拒绝授权时，返回明确的能力/权限结果和系统设置引导，不伪报已连接。

断开前仍需校验目标。重启后仅凭保存资料或“看到在线”不能自动恢复断开能力。若后续需要支持断开系统建立的连接，必须有单独的目标证明与明确用户动作；本轮不放宽当前限制。

### 5.3 只读 NetworkObservation

至少表达：Online/Offline/Unknown、观测范围、观测时间、相关本机接口/地址、账号关联是否得到证明，以及当前允许执行的操作。

状态属于当前网络环境。换 Wi-Fi、切换有线网、IP 改变、休眠恢复或观测过期后，需要新的只读证明。上次应用运行留下的 Online 不可以直接显示为本次已在线。

请求出口在线与本机物理网卡在线分别表达。在线来源不明确时不能推断为 srun 或 Tsinghua Secure。

两个 Auth 账号为空且本机 Online 是合法组合；两个账号有效但本机暂时 Offline 也不是密码过期的证据。

### 5.4 connect 的返回值是连接结果

```rust
// 设计示意。
pub enum NetworkConnectOutcome {
    Connected(NetworkObservation),
    AlreadyOnline(NetworkObservation),
    OutcomeUnconfirmed(NetworkOperationRef),
}
```

确定的错误通过结构化 `NetworkError` 返回，例如本地接口不可用、资料缺密码、密码被拒绝、限流或目标变化。`AlreadyOnline` 不承诺已用本次资料完成了新登录；观测中的账号关系需明确为匹配/不同/未确认。

如果已经在线，但账号不同或无法确认，不自动断开后切换账号。需要切换时由用户明确操作。提交结果不明确时可以通过受控只读查询确认，不能自动再次发送密码请求。

`NetworkConnectOutcome` 不会变成 `AuthStep::Authenticated`，也不改写两个账号的 `automatic_recovery_enabled`。

## 6. 保存、自动填写与连接三步分开

### 6.1 保存策略

用户可以选择不保存、只保存账号/连接方式、或者同时保存密码。Portal 与 `SystemWifiEap` 密码属于不同用途；密码保存是明确选项，不由“连接成功”自动推断。选择保存时由 TsinghuaKit 写入应用私有目录中的本地资料文件。TsinghuaKit 不读取、修改或判断系统 Wi-Fi 设置中已有的 Tsinghua Secure 凭据。

`profiles().save(...)`、选择默认资料和删除资料都只是本地操作。保存失败应明确告知；已经成功的网络连接不因本地保存失败被改写成“连接失败”。两项结果分别报告。

两个账号域的“保存凭据”“允许会话快照”“允许有界恢复”也分别表达，不能复用一个全局 `remember_credentials` 覆盖三个用途。统一身份的信任设备选项仍只按实际适用的认证协议提供。

### 6.2 明确触发后再填入密码

普通 `NetworkProfileSummary` 与 `PreparedNetworkInput` 只提供资料名称、账号、用途、密码是否已保存和版本引用，不包含密码。当前 Rust API 另外提供 `password_for_fill(&PreparedNetworkInput)`：只有同一 Client 的当前资料版本才能读取，结果是不可克隆/序列化、`Debug` 脱敏、drop 时零化的 `NetworkProfilePassword`。只有用户显式点击“填入”并调用 `expose_for_form()`，才把明文交给表单字段。Rust 保持秘密生命周期和版本核验；Flutter 表单不可避免会持有显示所需的字符串副本，因此 UI 应在表单结束或用户切换资料时清空字段，不能写日志或普通持久状态。

点击连接时应优先提交资料引用，由 Rust connector 在一次连接操作内读取秘密，而不是先回填再从 Flutter 读回。对于系统 Wi-Fi 设置，能否自动配置/提交取决于平台适配器与 OS 权限；不支持的平台应引导用户在 OS 中完成配置，不得声称 SDK 已把密码自动写进系统界面。任何需要把凭据交给平台层的路径都必须是用户显式发起、用途限定且不写日志。

用户手动输入新密码时，只以输入参数传给 Rust；输出结果、日志与事件不回传该值。

### 6.3 字段编辑规则

- 切换所选资料时，同时切换账号、连接方式与密码来源。
- 手动修改账号或连接方式后，解除原资料的隐式密码绑定；不能把旧资料密码静默提交给新账号/新协议。
- 只修改密码时，本次使用新输入，是否覆盖保存资料由显式保存操作决定。
- 资料被其他页面修改后，旧填写引用按版本校验；不静默使用另一个版本的账号/密码。
- 删除资料不会清除用户当前手动输入，也不会隐式断网；删除后旧的资料引用不能再读出密码。

### 6.4 Rust 侧调用形态

当前 SDK 已实现本机 Client 生命周期内的资料管理与显式密码填充；以下调用不联网：

```rust
let prepared = {
    let mut network = client.network();
    network.profiles().prepare_fill(profile_id)?
};

// UI 可从 prepared.summary() 填入账号。只有用户点按“填入密码”时才读取：
let password = {
    let mut network = client.network();
    network.profiles().password_for_fill(&prepared)?
};
if let Some(password) = password.as_ref() {
    password_field = password.expose_for_form().to_owned();
}
```

常规资料 DTO 不含口令。读取密码需要最新的同 Client 资料引用，且产生脱敏、不可克隆/序列化、drop 时零化的 Rust 值；把值复制到 Flutter 字段是用户显式填写动作的边界。跨进程持久化使用 SDK 管理的应用私有加密目录文件，密钥与加密文件同目录保存，不依赖 Flutter Secure Storage。Flutter facade 提供准备表单、显式密码句柄及 Portal connector；connector 在 Rust 内部读取资料密码或接收仅本次使用的手动密码，不要求先保存。当前没有 Tsinghua Secure 系统配置能力或自动连接流程。选择/填写默认资料不产生网络提交；本轮不把后台自动连接加入需求。

### 6.5 与 Tsinghua Secure 的关系

系统已经通过 Tsinghua Secure 接入时，不需要再补一次门户登录。当前 SDK 的只读检查仅查询 TUNet 门户对本机 IPv4 的登记状态；`PortalAddressRegistration::NotRegistered` 不代表本机没有互联网连接。操作系统仍是实际 Wi-Fi/802.1X 配置与 EAP 状态的权威来源，公共 SDK 目前没有通用互联网探测 API。

用户可以在 TsinghuaKit 的本地 `SystemWifiEap` 资料中选择保存独立的账号/密码并在应用表单中自动填写；这不等于操作系统已经保存或验证该凭据。当前是否能一键配置/连接必须由平台适配器按能力与权限报告。macOS、iOS、Android、Windows、Linux 的系统网络 API 和授权模型不同，不能仅凭跨平台 Rust API 就承诺一致的自动配置能力。

现有 srun 实现只接受 `Portal` 资料，不能把系统 EAP 密码自动送入门户请求。反向也不能把 Portal 凭据写入 Tsinghua Secure 的系统网络配置。切换/删除一个资料不影响另一个用途的凭据；删除 TsinghuaKit 资料也不声称清除了系统 Wi-Fi 中已有的配置。

## 7. 持久化按用途与账号隔离

### 7.1 逻辑键必须包含用途

建议使用逻辑存储键，而非按 username 直接命名全部记录：

```text
AuthCredential(host, Identity, account_ref)
AuthCredential(host, SelfService, account_ref)
NetworkProfileCredential(host, profile_id, Portal)
NetworkProfileCredential(host, profile_id, SystemWifiEap)
```

`account_ref` 的构造必须包含认证域；相同字符串的 Identity(A) 与 SelfService(A) 仍是两个不同作用域。外部文件名使用不暴露账号的记录标识。

记录加密时将 schema、宿主、用途和记录标识纳入已认证的元信息。不能把旧 Identity 密文直接作为 USEREG 或网络资料来解释，也不能靠相同 username 做自动凭据回退。

可以共用同一 Rust 私有存储后端，但需要分别保存以下对象：

| 对象 | 命名空间/归属 | 是否能当作在线证明 |
| --- | --- | --- |
| Identity 账号元信息、保存偏好、注销权威 | 宿主 + Identity + 账号 | 否 |
| SelfService 账号元信息、保存偏好、注销权威 | 宿主 + SelfService + 账号 | 否 |
| 两域的可选密码 | 各自域/账号的私有凭据记录 | 否 |
| 受控 transport/会话快照 | 共享会话管理器，附账号/通道绑定 | 恢复后仍需当前证明 |
| 业务缓存 | 所属 Auth 域 + 账号 + 业务查询 | 只证明保存时的业务数据，不能证明当前在线 |
| NetworkProfile 与可选密码 | 宿主本机资料作用域，按 Portal/EAP 用途隔离 | 否 |
| NetworkObservation / ConnectionTargetRef | 进程内、当前网络环境 | 仅在当前验证范围和有效期内使用，不作为可续期 Auth 会话 |

新增命名空间继续复用私有目录、权限、符号链接、文件锁和加密格式校验。公开 ProfileId 只选择受管记录，不能变成可任意指定的密码文件路径。

### 7.2 Cookie 共享与账号隔离同时成立

当前 USEREG 经 WebVPN 的路径需要共享访问通道，不能为了两个账号目录就复制成两个互不相关的 Cookie jar。

Runtime 内仍只有一个共享 transport/Cookie jar，以满足当前 WebVPN handoff 路径。SDK 现有的可选目录快照只在 Identity 首次成功持久化时保存一次当时的整个 jar；之后响应带来的 Cookie 不会写回此快照。它降低了 SelfService 和后续业务 Cookie 混入恢复快照的机会，但不构成逐 Cookie 的账号归属或分区，而且保存时 jar 中已有的 Cookie 仍可能入快照。新 Client 仅把快照作为 `RestoredUnverified` 的 Identity 恢复候选，不恢复 SelfService 会话或授权；旧 envelope schema 1 会被拒绝，不做静默迁移。

这是迁移期边界，不是最终的多域凭据存储。后续存储实现仍须为 Identity、SelfService、共享访问通道及派生服务证明明确记录主体、域和撤销代次；在协议无法证明单个 Cookie 的归属前，不能把共享 jar 的序列化快照描述为完整的账号隔离。

恢复时检查账号域、主体、通道上下文和注销权威；不能因为恢复了某个 Cookie，就把两个账号一起标成已认证。

注销 B 时清除/撤销 B 的证明和账号选择，并使其挑战、设备引用失效；保留 A/门户需要的共享 transport。Cookie 无法可靠独立归属时，依赖明确的逻辑撤销与重新验证，不能按 `webvpn.tsinghua.edu.cn` 一刀切删掉所有 Cookie，也不能让已注销 B 因残留 Cookie 自动恢复。注销 A 时必须同时废止派生服务证明与依赖旧访问通道的 B 在线证明；若 B 仍处于所选状态，则报告 `Expired`，等待重新建立 Identity/WebVPN 通道后再验证 B。

### 7.3 三种清理动作分别实现

- **注销账号**：先撤销运行中和持久恢复权威、阻断晚到结果；保留已显式保存的填写资料供下一次用户主动登录。注销本身不授权后台再次用保存密码登录。`identity.logout()` 同时使依赖 A 的服务证明失效，并把仍被选择的 B 标为 `Expired`；`self_service.logout()` 清除 B 的账号选择、会话与挑战，保留 A 和共享 transport；`auth.logout_all()` 清除两个账号域。旧兼容 `CampusRuntime::logout()` 继续是全局重置。
- **遗忘账号**：注销后移除对应域/账号的保存凭据、账号资料及其受管本地业务数据，不清理另一域和本机连接资料。
- **删除网络连接资料**：移除指定 Profile 与密码记录，使旧引用失效；不发送校园网断开请求。

UI 如需“清除本机所有资料”，提供范围明确的单独动作。旧 `CampusRuntime::logout` 的兼容适配仍按旧版本的全局重置/凭据清理语义实现；新版单域 `logout` 的保留规则必须在迁移文档中注明，不能静默改变旧调用者行为。

文件清理失败时，已经注销的在线权限不能重新生效；持久注销权威继续阻止恢复，同时向调用者报告未完成的清理。不把物理删除失败显示为“全部已清除”。

## 8. 操作影响矩阵

以下是新 API 的默认语义。是否保存密码始终由各用途的显式选择决定。

| 操作 | Identity | SelfService | 校园网资料 | 本机网络连接 |
| --- | --- | --- | --- | --- |
| 登录/切换 Identity | 创建新账号/会话代次，失效旧派生上下文 | 资料保留；依赖旧访问通道的在线证明待重证 | 保留 | 不自动连接/断开 |
| Identity 明确过期 | 自己及依赖它的证明不可用 | 资料保留；通道受影响时访问阻塞/重证 | 保留 | 只按网络观测判断 |
| 注销 Identity | 退出并撤销所有派生证明，禁止后台恢复 | 若 B 仍被选中则保留选择并置为 `Expired`，旧通道恢复后须重证 | 保留 | 不断网 |
| 遗忘 Identity | 清理此域受管记录 | 独立资料保留，处理通道失效 | 保留 | 不断网 |
| 登录/切换 SelfService | 不重登 A、不覆盖 A 的密码 | 更换自己的账号/会话代次和引用 | 保留 | 不自动连接/断开 |
| 注销 SelfService | 保持有效的 A 与其他业务 | 清除 B 当前选择、会话、验证码挑战及设备目标 | 保留 | 不断网 |
| 遗忘 SelfService | 保持有效的 A 与其他业务 | 注销并清除 B 的受管资料与保存凭据 | 保留 | 不断网 |
| `auth().logout_all()` | 退出并撤销此域及派生证明 | 退出并撤销此域 | 保留 | 不断网 |
| 保存/选择/自动填写 NetworkProfile | 不变 | 不变 | 更新所选本机资料 | 不发送连接请求；OS 配置不随之改变 |
| 显式 `network().connect()` | 不创建/刷新身份会话 | 不创建/刷新自助会话 | 仅在独立保存动作下写入 | 按资料方式提交 Portal 或受支持的系统网络操作并证明结果 |
| 显式 `network().disconnect()` | 业务可能暂时无法联网，但不直接判账号过期 | 同左 | 保留 | 校验目标后断开 |
| 删除 NetworkProfile | 不变 | 不变 | 删除指定记录 | 已连接状态不变 |
| 关闭/重启 App | 按保存策略处理，不能伪称内存会话存活 | 同左且考虑通道前置条件 | 按保存选择保留 | OS/服务端状态独立；重新观测，不自动连接 |

USEREG 的 `self_service().disconnect_device(DeviceRef)` 可能断开其他在线设备，属于 B 名下的业务操作。它不能复用 `network().disconnect` 的本机目标引用，也不能由网络状态页面自动执行。

## 9. 应用启动和界面状态

### 9.1 启动流程

1. 构造一个 Client，加载已允许持久化的本地元信息；不发认证/连接请求。
2. 分别发布 Identity、SelfService 的资料与未验证状态；网络页面加载所选连接资料的填写摘要。
3. 按应用既定读取时机查询本机网络状态；此查询不要求 Auth 已登录。
4. 在对应域允许恢复且没有注销阻断时，执行有界账号恢复；USEREG 路由缺前置证明时返回明确依赖状态，不隐式向用户保存的另一账号反复要密码。
5. 网络连接只有用户明确点击后执行。填好了密码、页面打开了、账号恢复失败了，都不自动触发连接。

上述步骤中，网络观测与账号业务读可以遵守现有的有界队列；实际认证、一次性提交与可变网络操作仍经过统一调度与独占要求。

### 9.2 合法的状态组合

| 状态组合 | UI 应当如何理解 |
| --- | --- |
| 两个 Auth 槽均空，本机 Online | 系统/网络已在线，校园业务尚未登录 |
| Identity(A) 已验证，B 未登录，本机 Online | A 的可用业务可读，USEREG 显示自己的登录入口 |
| Identity(A) 与 SelfService(B) 已验证 | 同时展示两个实际账号；A/B 不同是正常配置 |
| 保存了 B，但 A/门户前置条件缺失 | B 的资料可展示/填写，在线自助访问等待前置证明 |
| 两个账号仍有会话记录，本机 Offline | 暂时网络不可用；不能只因断网把密码标为错误 |
| 只有 NetworkProfile，没有在线证明 | 表单可自动填写；网络显示未检查/Unknown，而非已连接 |

### 9.3 Flutter 与 Rust 的边界

Flutter 分别维护 IdentityAuth、SelfServiceAuth、NetworkConnection 和 NetworkProfile 的展示状态，但它们共享一个 Rust gateway/Client。

`AuthStatusDto` 包含两个账号状态；网络操作返回 `NetworkObservationDto`/`NetworkConnectResultDto`，不再返回主账号的 `CampusRuntimeStatusDto`。这样校园网按钮是否成功直接取决于连接结果，不依赖 App 登录状态是否变化。

页面应区分“统一身份”“网络自助账号”和“校园网连接资料”。本机网络入口在未登录 App 账号时也可以使用。USEREG 显示的是自己的业务账号及所需前置条件，而不是照抄主账号名。

认证正确性、Cookie、恢复策略、资料密文读取和实际网络操作留在 Rust。Flutter 负责输入、选择资料、显式提交和渲染返回的结构化状态。

## 10. 落地顺序

1. **先定域和返回类型**：`AuthDomain` 只有两项；定义双账号状态、本机连接资料、操作结果与观测类型，更新 Rustdoc 和方法迁移表。
2. **保留并显式化 USEREG 双主体保护**：拆出其账号槽与代次，保留 `identity_owner` 所代表的访问通道约束；补齐 A/B 不同与同 username 不同用途的契约。
3. **把 TUNet 从 Auth 注册/恢复链移出**：将当前 username/IP 状态改为受限短期连接上下文；成功后返回连接结果，不写 `Authenticated`。网络读取也不刷新身份恢复快照。
4. **增加本机连接资料存储和填写接口（首批完成）**：Rust 已分开建模 Portal 与 `SystemWifiEap`、可选密码、资料版本与显式 `NetworkProfilePassword` 填写边界；默认 memory-only。持久化由 SDK 管理应用私有目录加密文件；不接入系统凭据存储。待完成：App 接入、默认资料选择和产品 UI；保存/填写本身不联网，也不改写 OS Wi-Fi 配置。
5. **拆分注销和恢复**：两个域分别维护权威，加入上述依赖失效矩阵；保留旧 bridge 的明确兼容包装。
6. **更新 Flutter/CLI 消费与生成代码（进行中）**：新的 FRB 绑定与 Dart facade 已生成并覆盖 Auth 和本地资料首批方法，macOS 插件构建及无登录启动 smoke test 通过。THYou App 仍在 `v0.1.1` 旧 Runtime facade；在其它业务方法也迁入并通过同一 Client 提供前，不应在 App 中同时维护新旧 Rust Client。后续分别消费双账号状态和连接结果，系统 Wi-Fi 每个平台显式报告连接/配置能力和授权结果。只读网络定向用例不人为依赖两个账号都登录；全量验收的执行顺序不等于能力本身的认证依赖。
7. **迁移保存格式并发布候选版本**：旧明确属于 Identity 的凭据只迁移到 Identity；没有已保存的 USEREG/网络密码就保持缺省，不能从 Identity 推测填充。持久化过的旧 TUNet 标志不能当作新账号会话或本次在线证明。

`tunet` 可以继续作为遥测、功能目录和 CLI 用例的标识。迁出 Auth 指的是取消其持久账号/恢复语义，不是删除所有带 `tunet` 名称的协议实现或记录。

## 11. 新增验证要求

以下大多是未来实施时的新增契约，不是本轮已执行的验证结果。`DeviceRef` 的创建 Client/最新列表绑定、Debug 脱敏、刷新/失败/注销/成功断开失效及跨 Client 拒绝，已由本地 fixture 和外部 SDK 编译用例验证：

- Identity(A) + SelfService(B) 且 A 不等于 B 可以正确登录和分别读数据；USEREG 响应必须匹配 B。
- 两域和 NetworkProfile 使用同一个 username、三个不同密码时，保存、填写、修改和遗忘互不覆盖。
- Identity、SelfService、Portal 与 SystemWifiEap 使用相同 username、不同密码时，四种用途仍由独立凭据键隔离。
- 切换 A 后，USEREG 旧挑战/设备句柄不能越过通道代次；B 的保存资料仍在。
- 切换/注销 B 不误清 A/门户 Cookie，也不重登 A；残留 Cookie 不能恢复已明确注销的 B。
- 两个 Auth 槽为空时仍能只读查询网络；本机通过 Secure 在线不要求 srun 登录。
- `connect` 成功、失败或结果不明确均不写入两个 Auth 状态或身份恢复快照。
- 保存、选择、自动填写资料和重启不会发送连接请求；本地保存失败与网络操作成功分别表达。
- 选择 `SystemWifiEap` 资料不会调用 srun；不支持的 OS/平台能力返回明确结果，不能伪称自动配置或已连接。
- TsinghuaKit 的 EAP 资料编辑/删除不会覆盖系统原生 Wi-Fi 凭据；系统原生配置的变更只能由显式授权的平台适配器报告。
- 修改账号/方式后不带入旧资料密码；资料删除/版本变化后旧引用失效。
- 连接前后接口/IP 变化时，旧断开目标被拒绝；仅观察 Online 不取得断开能力。
- 网络断开/短期故障不直接变成两个账号的认证过期，也不触发密码自动重放。
- 单域注销与全域注销写入正确作用域的持久权威，清理失败也不能复活会话。
- 生成 DTO、Debug、日志与报告均无保存密码、Cookie、临时 challenge 或其他认证材料。

真实验证沿用现有只读、单进程、单一 Rust Runtime 和新增/失败用例台账。`DeviceRef` 仅在 loopback fixture 中验证了选择、失效和一次合成断开，没有访问真实设备或执行线上操作；校园网络连接/断开、账号登录和可变真实设备操作均未执行。
