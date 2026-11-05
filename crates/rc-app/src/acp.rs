//! ACP v1 stdio client. Agent startup and prompts are explicit; delegated filesystem and
//! terminal capabilities are disabled, and every supported permission requires a user choice.
use rc_core::{Inbox,Waker,inbox::Outbox};
use rc_kit::i18n::{tr,trf};
use serde_json::{Value,json};
use std::{collections::BTreeMap,path::PathBuf,process::Stdio,time::Duration};
use tokio::{io::{AsyncBufReadExt,AsyncRead,AsyncWriteExt,BufReader},sync::mpsc,time::Instant};
const FRAME:usize=1024*1024;
const TRAFFIC:usize=16*1024*1024;
#[derive(Clone,Debug)]
pub enum Error{Trust,Input,Io,Protocol,Version,Limit,Busy,NotReady,Cancelled,CancelTimeout,Remote{i:i64,message:String}}
impl Error{fn message(&self)->String{match self{Self::Trust=>tr("Trust this project before running project tools or code").into(),Self::Input=>tr("Choose an ACP agent executable and at most 32 separate arguments").into(),Self::Io=>tr("Could not start or communicate with the ACP agent").into(),Self::Protocol=>tr("The agent sent invalid or unsupported ACP JSON-RPC data").into(),Self::Version=>tr("The agent did not negotiate supported ACP protocol version 1").into(),Self::Limit=>tr("ACP message, traffic, queue, time or display limits were exceeded").into(),Self::Busy=>tr("Wait for the current ACP request to finish").into(),Self::NotReady=>tr("Start an ACP agent and create a session before sending a prompt").into(),Self::Cancelled=>tr("ACP agent disconnected").into(),Self::CancelTimeout=>tr("The ACP agent did not acknowledge cancellation; its process tree was stopped").into(),Self::Remote{i,message}=>trf("ACP agent error {code}: {message}",&[("code",i),("message",message)])}}}
struct Frames<R:AsyncRead+Unpin>{reader:BufReader<R>,partial:Vec<u8>,total:usize,count:usize}
impl<R:AsyncRead+Unpin> Frames<R>{fn new(reader:R)->Self{Self{reader:BufReader::new(reader),partial:Vec::new(),total:0,count:0}}
    async fn next(&mut self)->Result<Value,Error>{loop{let bytes=self.reader.fill_buf().await.map_err(|_|Error::Io)?;if bytes.is_empty(){return Err(Error::Io);}let end=bytes.iter().position(|b|*b==b'\n');let n=end.map_or(bytes.len(),|n|n+1);if self.partial.len()+n>FRAME||self.total+n>TRAFFIC||self.count>=10000{return Err(Error::Limit);}self.partial.extend_from_slice(&bytes[..n]);self.total+=n;self.reader.consume(n);if end.is_some(){self.count+=1;let bytes=std::mem::take(&mut self.partial);let value:Value=serde_json::from_slice(&bytes).map_err(|_|Error::Protocol)?;if value.get("jsonrpc").and_then(Value::as_str)!=Some("2.0")||!value.is_object(){return Err(Error::Protocol);}return Ok(value);}}}
}
async fn send(stdin:&mut tokio::process::ChildStdin,value:Value,stop:&rc_dotnet::CancelToken)->Result<(),Error>{let mut bytes=serde_json::to_vec(&value).map_err(|_|Error::Protocol)?;if bytes.len()>FRAME{return Err(Error::Limit);}bytes.push(b'\n');tokio::select!{result=tokio::time::timeout(Duration::from_secs(2),stdin.write_all(&bytes))=>result.map_err(|_|Error::Io)?.map_err(|_|Error::Io),()=stop.cancelled()=>Err(Error::Cancelled)}}
#[derive(Clone,Debug)]
pub struct Auth{pub id:String,pub name:String,pub supported:bool}
#[derive(Clone,Debug)]
pub struct OptionItem{pub id:String,pub name:String,pub kind:String}
#[derive(Clone,Debug)]
pub struct Permission{pub id:Value,pub details:String,pub options:Vec<OptionItem>}
fn bounded(value:&Value,key:&str,max:usize)->Result<String,Error>{let text=value.get(key).and_then(Value::as_str).ok_or(Error::Protocol)?;if text.is_empty()||text.len()>max{return Err(Error::Limit);}Ok(text.into())}
fn request_id(id:&Value)->Result<String,Error>{if id.as_i64().is_some()||id.as_u64().is_some(){return Ok(id.to_string());}if id.as_str().is_some_and(|id|!id.is_empty()&&id.len()<=128){return Ok(id.to_string());}Err(Error::Protocol)}
fn permission(id:Value,params:&Value)->Result<Permission,Error>{request_id(&id)?;let details=serde_json::to_string(params.get("toolCall").ok_or(Error::Protocol)?).map_err(|_|Error::Protocol)?;if details.len()>64*1024{return Err(Error::Limit);}let rows=params.get("options").and_then(Value::as_array).ok_or(Error::Protocol)?;if rows.is_empty()||rows.len()>8{return Err(Error::Limit);}let mut options=Vec::new();let mut ids=std::collections::HashSet::new();for row in rows{let id=bounded(row,"optionId",128)?;if !ids.insert(id.clone()){return Err(Error::Protocol);}let name=bounded(row,"name",1024)?;let kind=bounded(row,"kind",32)?;if !matches!(kind.as_str(),"allow_once"|"reject_once"|"allow_always"|"reject_always"){return Err(Error::Protocol);}options.push(OptionItem{id,name,kind});}Ok(Permission{id,details,options})}
#[derive(Clone,Debug)]
pub enum Event{Agent{name:String,auth:Vec<Auth>},Session(String),Busy(bool),Prompt(bool),Text(String),Tool(String),Log(String),Permission(Permission),Gone(Value),ClearPermissions,Error(Error),Stopped}
#[derive(Debug)]
enum Command{Prompt(String),NewSession,Authenticate(String),Choose{id:Value,option:String},CancelPrompt}
pub struct Handle{commands:mpsc::Sender<Command>,stop:rc_dotnet::CancelToken}
impl Handle{fn command(&self,command:Command)->Result<(),Error>{self.commands.try_send(command).map_err(|_|Error::Limit)}pub fn stop(&self){self.stop.cancel();}}
impl Drop for Handle{fn drop(&mut self){self.stop();}}
#[derive(Clone,Copy,PartialEq)]
enum Waiting{Initialize,Session,Auth,Prompt}
struct Pending{id:u64,kind:Waiting,deadline:Instant}
struct Engine{session:Option<String>,pending:Option<Pending>,next:u64,auth:Vec<Auth>,permissions:BTreeMap<String,Permission>,cancelling:Option<Instant>,early:Vec<Value>,root:String,initialized:bool}
impl Engine{
    async fn call(&mut self,method:&str,params:Value,kind:Waiting,stdin:&mut tokio::process::ChildStdin,stop:&rc_dotnet::CancelToken,out:&Outbox<Event>)->Result<(),Error>{if self.pending.is_some(){return Err(Error::Busy);}if self.next>=10000{return Err(Error::Limit);}self.next+=1;let id=self.next;send(stdin,json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),stop).await?;self.pending=Some(Pending{id,kind,deadline:Instant::now()+Duration::from_secs(match kind{Waiting::Prompt=>300,Waiting::Auth=>120,_=>30})});out.send(Event::Busy(true));if kind==Waiting::Prompt{out.send(Event::Prompt(true));}Ok(())}
    async fn new_session(&mut self,stdin:&mut tokio::process::ChildStdin,stop:&rc_dotnet::CancelToken,out:&Outbox<Event>)->Result<(),Error>{if !self.initialized{return Err(Error::NotReady);}if self.pending.is_some(){return Err(Error::Busy);}self.session=None;self.early.clear();self.call("session/new",json!({"cwd":self.root,"mcpServers":[]}),Waiting::Session,stdin,stop,out).await}
    async fn clear_permissions(&mut self,stdin:&mut tokio::process::ChildStdin,stop:&rc_dotnet::CancelToken,out:&Outbox<Event>)->Result<(),Error>{for permission in std::mem::take(&mut self.permissions).into_values(){send(stdin,json!({"jsonrpc":"2.0","id":permission.id,"result":{"outcome":{"outcome":"cancelled"}}}),stop).await?;}out.send(Event::ClearPermissions);Ok(())}
    fn update(&self,params:&Value,out:&Outbox<Event>)->Result<(),Error>{if params.get("sessionId").and_then(Value::as_str)!=self.session.as_deref(){return Err(Error::Protocol);}let update=params.get("update").ok_or(Error::Protocol)?;match update.get("sessionUpdate").and_then(Value::as_str){Some("agent_message_chunk")=>{let content=update.get("content").ok_or(Error::Protocol)?;if content.get("type").and_then(Value::as_str)==Some("text"){let text=content.get("text").and_then(Value::as_str).ok_or(Error::Protocol)?;if text.len()>64*1024{return Err(Error::Limit);}out.send(Event::Text(text.into()));}},Some("tool_call"|"tool_call_update"|"plan")=>{let text=serde_json::to_string(update).map_err(|_|Error::Protocol)?;if text.len()>64*1024{return Err(Error::Limit);}out.send(Event::Tool(text));},_=>{}}Ok(())}
    async fn incoming(&mut self,value:Value,stdin:&mut tokio::process::ChildStdin,stop:&rc_dotnet::CancelToken,out:&Outbox<Event>)->Result<(),Error>{
        if value.get("method").is_some_and(|method|!method.is_string()){return Err(Error::Protocol);}
        if let Some(method)=value.get("method").and_then(Value::as_str){let params=value.get("params").cloned().unwrap_or(Value::Null);if let Some(id)=value.get("id").cloned(){request_id(&id)?;if method=="session/request_permission"{if self.pending.as_ref().is_none_or(|p|p.kind!=Waiting::Prompt)||params.get("sessionId").and_then(Value::as_str)!=self.session.as_deref()||self.cancelling.is_some(){send(stdin,json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}}),stop).await?;return Ok(());}let permission=permission(id.clone(),&params)?;let key=request_id(&id)?;if self.permissions.len()>=8||self.permissions.contains_key(&key){return Err(Error::Limit);}self.permissions.insert(key,permission.clone());out.send(Event::Permission(permission));}else{send(stdin,json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client capability is unsupported"}}),stop).await?;}}else if method=="session/update"{if self.pending.as_ref().is_some_and(|p|p.kind==Waiting::Session)&&self.session.is_none(){if self.early.len()>=16{return Err(Error::Limit);}self.early.push(params);}else{self.update(&params,out)?;}}else if method=="$/cancel_request"{if let Some(id)=params.get("id"){let key=request_id(id)?;if let Some(permission)=self.permissions.remove(&key){send(stdin,json!({"jsonrpc":"2.0","id":permission.id,"result":{"outcome":{"outcome":"cancelled"}}}),stop).await?;out.send(Event::Gone(permission.id));}}}return Ok(());}
        if value.get("result").is_some()==value.get("error").is_some(){return Err(Error::Protocol);}
        let id=value.get("id").and_then(Value::as_u64).ok_or(Error::Protocol)?;if self.pending.as_ref().is_none_or(|pending|pending.id!=id){return Err(Error::Protocol);}let pending=self.pending.take().ok_or(Error::Protocol)?;out.send(Event::Busy(false));
        if let Some(error)=value.get("error"){let i=error.get("code").and_then(Value::as_i64).unwrap_or(-32603);let message=error.get("message").and_then(Value::as_str).unwrap_or("").chars().take(2048).collect();if pending.kind==Waiting::Prompt{self.clear_permissions(stdin,stop,out).await?;self.cancelling=None;out.send(Event::Prompt(false));}if pending.kind==Waiting::Initialize{return Err(Error::Remote{i,message});}if i!=-32800||pending.kind!=Waiting::Prompt{out.send(Event::Error(Error::Remote{i,message}));}return Ok(());}
        let result=value.get("result").ok_or(Error::Protocol)?;
        match pending.kind{
            Waiting::Initialize=>{if result.get("protocolVersion").and_then(Value::as_u64)!=Some(1){return Err(Error::Version);}let name=result.get("agentInfo").and_then(|v|v.get("name")).and_then(Value::as_str).unwrap_or("ACP").chars().take(256).collect();let rows=result.get("authMethods").and_then(Value::as_array);if rows.is_some_and(|rows|rows.len()>16){return Err(Error::Limit);}if let Some(rows)=rows{for row in rows{self.auth.push(Auth{id:bounded(row,"id",128)?,name:bounded(row,"name",256)?,supported:row.get("type").and_then(Value::as_str).is_none_or(|kind|kind=="agent")});}}self.initialized=true;out.send(Event::Agent{name,auth:self.auth.clone()});self.new_session(stdin,stop,out).await?;},
            Waiting::Session=>{let id=bounded(result,"sessionId",512)?;self.session=Some(id.clone());out.send(Event::Session(id));for update in std::mem::take(&mut self.early){self.update(&update,out)?;}},
            Waiting::Auth=>self.new_session(stdin,stop,out).await?,
            Waiting::Prompt=>{let reason=bounded(result,"stopReason",64)?;self.clear_permissions(stdin,stop,out).await?;self.cancelling=None;out.send(Event::Prompt(false));out.send(Event::Tool(format!("{}: {reason}",tr("ACP prompt finished"))));},
        }Ok(())
    }
    async fn command(&mut self,command:Command,stdin:&mut tokio::process::ChildStdin,stop:&rc_dotnet::CancelToken,out:&Outbox<Event>)->Result<(),Error>{match command{
        Command::Prompt(text)=>{if text.is_empty()||text.len()>64*1024{return Err(Error::Limit);}let session=self.session.clone().ok_or(Error::NotReady)?;self.call("session/prompt",json!({"sessionId":session,"prompt":[{"type":"text","text":text}]}),Waiting::Prompt,stdin,stop,out).await},
        Command::NewSession=>self.new_session(stdin,stop,out).await,
        Command::Authenticate(id)=>{if !self.auth.iter().any(|auth|auth.id==id&&auth.supported){return Err(Error::Input);}self.call("authenticate",json!({"methodId":id}),Waiting::Auth,stdin,stop,out).await},
        Command::Choose{id,option}=>{let key=request_id(&id)?;let permission=self.permissions.get(&key).ok_or(Error::NotReady)?;if !permission.options.iter().any(|item|item.id==option&&matches!(item.kind.as_str(),"allow_once"|"reject_once")){return Err(Error::Input);}send(stdin,json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":option}}}),stop).await?;self.permissions.remove(&key);out.send(Event::Gone(id));Ok(())},
        Command::CancelPrompt=>{if self.pending.as_ref().is_none_or(|p|p.kind!=Waiting::Prompt){return Err(Error::NotReady);}self.clear_permissions(stdin,stop,out).await?;send(stdin,json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.session}}),stop).await?;self.cancelling=Some(Instant::now()+Duration::from_secs(2));Ok(())},
    }}
}
struct Group(Option<u32>);impl Drop for Group{fn drop(&mut self){if let Some(pid)=self.0{rc_dotnet::kill_tree(pid);}}}
pub fn launch(program:String,args:Vec<String>,root:PathBuf,trusted:bool,out:Outbox<Event>)->Result<Handle,Error>{if !trusted{return Err(Error::Trust);}if program.is_empty()||program.len()>4096||program.contains('\0')||args.len()>32||args.iter().any(|arg|arg.len()>4096||arg.contains('\0'))||(!PathBuf::from(&program).is_absolute()&&PathBuf::from(&program).components().count()>1){return Err(Error::Input);}if !root.is_absolute(){return Err(Error::Input);}let cwd=root.to_str().ok_or(Error::Input)?.to_string();let(tx,rx)=mpsc::channel(16);let stop=rc_dotnet::CancelToken::new();let worker_stop=stop.clone();rc_core::Runtime::spawn(async move{worker(program,args,root,cwd,rx,worker_stop,out).await;});Ok(Handle{commands:tx,stop})}
async fn worker(program:String,args:Vec<String>,root:PathBuf,cwd:String,mut commands:mpsc::Receiver<Command>,stop:rc_dotnet::CancelToken,out:Outbox<Event>){
    let work=async{
        if stop.is_cancelled(){return Err(Error::Cancelled);}let root=root.canonicalize().map_err(|_|Error::Input)?;if stop.is_cancelled(){return Err(Error::Cancelled);}
        let mut cmd=tokio::process::Command::new(program);cmd.args(args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);#[cfg(unix)]cmd.process_group(0);#[cfg(windows)]cmd.creation_flags(0x0800_0000);
        let mut child=cmd.spawn().map_err(|_|Error::Io)?;let mut group=Group(child.id());let stdout=child.stdout.take().ok_or(Error::Io)?;let mut stdin=child.stdin.take().ok_or(Error::Io)?;let mut stderr=child.stderr.take().ok_or(Error::Io)?;let log_out=out.clone();let log_stop=stop.clone();
        let log=tokio::spawn(async move{let mut total=0usize;let mut bytes=[0u8;4096];loop{match tokio::io::AsyncReadExt::read(&mut stderr,&mut bytes).await{Ok(0)|Err(_)=>break,Ok(n)=>{total+=n;if total>64*1024{log_out.send(Event::Error(Error::Limit));log_stop.cancel();break;}log_out.send(Event::Log(String::from_utf8_lossy(&bytes[..n]).into_owned()));}}}});
        let mut engine=Engine{session:None,pending:None,next:0,auth:Vec::new(),permissions:BTreeMap::new(),cancelling:None,early:Vec::new(),root:cwd,initialized:false};let mut frames=Frames::new(stdout);
        let run:Result<(),Error>=async{
            engine.call("initialize",json!({"protocolVersion":1,"clientInfo":{"name":"crabcraft","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"auth":{"terminal":false}}}),Waiting::Initialize,&mut stdin,&stop,&out).await?;
            loop{let deadline=engine.cancelling.or_else(||engine.pending.as_ref().map(|p|p.deadline)).unwrap_or_else(||Instant::now()+Duration::from_secs(86400));tokio::select!{
                ()=stop.cancelled()=>return Err(Error::Cancelled),
                result=frames.next()=>engine.incoming(result?,&mut stdin,&stop,&out).await?,
                command=commands.recv()=>{let Some(command)=command else{return Err(Error::Cancelled);};if let Err(error)=engine.command(command,&mut stdin,&stop,&out).await{if matches!(error,Error::Busy|Error::NotReady|Error::Input){out.send(Event::Error(error));}else{return Err(error);}}},
                ()=tokio::time::sleep_until(deadline)=>return Err(if engine.cancelling.is_some(){Error::CancelTimeout}else{Error::Limit}),
            }}
        }.await;
        // Resolve outstanding permissions as cancelled before stopping the process tree.
        let mut cleanup=Vec::new();for permission in engine.permissions.values(){if let Ok(mut bytes)=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":permission.id,"result":{"outcome":{"outcome":"cancelled"}}})){cleanup.append(&mut bytes);cleanup.push(b'\n');}}
        if let Some(session)=&engine.session{if let Ok(mut bytes)=serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})){cleanup.append(&mut bytes);cleanup.push(b'\n');}}
        let _=tokio::time::timeout(Duration::from_millis(250),stdin.write_all(&cleanup)).await;out.send(Event::ClearPermissions);if let Some(pid)=group.0.take(){rc_dotnet::kill_tree(pid);}let _=child.start_kill();let _=tokio::time::timeout(Duration::from_secs(1),child.wait()).await;log.abort();run
    }.await;
    if let Err(error)=work{let error=if stop.is_cancelled()&&!matches!(error,Error::Limit|Error::Protocol|Error::Version|Error::CancelTimeout){Error::Cancelled}else{error};out.send(Event::Error(error));}out.send(Event::Stopped);
}

pub struct Acp{
    pub program:String,pub args:String,pub input:String,pub transcript:String,pub tools:String,pub logs:String,pub agent:String,pub session:Option<String>,pub auth:Vec<Auth>,pub permissions:Vec<Permission>,pub busy:bool,pub running:bool,
    inbox:Inbox<Event>,handle:Option<Handle>,error:Option<String>,cancelling:bool,
}
impl Acp{
    pub fn new(waker:Waker)->Self{Self{program:String::new(),args:String::new(),input:String::new(),transcript:String::new(),tools:String::new(),logs:String::new(),agent:String::new(),session:None,auth:Vec::new(),permissions:Vec::new(),busy:false,running:false,inbox:Inbox::new(waker),handle:None,error:None,cancelling:false}}
    pub fn stop(&mut self){if let Some(handle)=&self.handle{handle.stop();}self.permissions.clear();self.cancelling=true;}
    fn command(&mut self,command:Command){let result=self.handle.as_ref().ok_or(Error::NotReady).and_then(|handle|handle.command(command));if let Err(error)=result{self.error=Some(error.message());if matches!(error,Error::Limit){self.stop();}}}
    fn start(&mut self,root:PathBuf,trusted:bool){if self.handle.is_some(){return;}self.error=None;self.cancelling=false;self.agent.clear();self.auth.clear();self.session=None;self.busy=true;let args=self.args.lines().filter(|line|!line.is_empty()).map(str::to_string).collect();match launch(self.program.clone(),args,root,trusted,self.inbox.sender()){Ok(handle)=>self.handle=Some(handle),Err(error)=>{self.busy=false;self.error=Some(error.message());}}}
    fn append(target:&mut String,text:&str,limit:usize)->Result<(),Error>{if target.len()+text.len()>limit{return Err(Error::Limit);}target.push_str(text);Ok(())}
    pub fn poll(&mut self,trusted:bool)->bool{if !trusted&&self.handle.is_some(){self.stop();self.error=Some(Error::Trust.message());}let mut permission_arrived=false;for event in self.inbox.drain(){let result=match event{
        Event::Agent{name,auth}=>{self.agent=name;self.auth=auth;Ok(())},Event::Session(session)=>{self.session=Some(session);self.busy=false;self.cancelling=false;Ok(())},Event::Busy(busy)=>{self.busy=busy;Ok(())},Event::Prompt(running)=>{self.running=running;if !running{self.cancelling=false;}Ok(())},Event::Text(text)=>Self::append(&mut self.transcript,&text,1024*1024),Event::Tool(text)=>Self::append(&mut self.tools,&format!("{text}\n"),256*1024),Event::Log(text)=>Self::append(&mut self.logs,&text,64*1024),Event::Permission(permission)=>{if !self.cancelling{if self.permissions.len()>=8{Err(Error::Limit)}else{self.permissions.push(permission);permission_arrived=true;Ok(())}}else{Ok(())}},Event::Gone(id)=>{self.permissions.retain(|permission|permission.id!=id);Ok(())},Event::ClearPermissions=>{self.permissions.clear();Ok(())},Event::Error(error)=>{if self.error.is_none()||!matches!(error,Error::Cancelled){self.error=Some(error.message());}Ok(())},Event::Stopped=>{self.handle=None;self.busy=false;self.running=false;self.session=None;self.permissions.clear();self.cancelling=false;Ok(())}};if let Err(error)=result{self.error=Some(error.message());self.stop();}}permission_arrived}
    pub fn show(&mut self,ui:&mut egui::Ui,root:Option<PathBuf>,trusted:bool){
        egui::ScrollArea::vertical().id_salt("acp_body").show(ui,|ui|{
            ui.label(tr("ACP v1 text client. Only start agents you trust: the external process has your OS permissions and may use network or execute tools."));
            ui.label(tr("No agent starts on project open. Client filesystem and terminal delegation, attachments, persistent permissions and session replay are unsupported."));
            if !trusted{ui.colored_label(rc_kit::pal(ui).warn,tr("Trust this project before running project tools or code"));}
            ui.add_enabled_ui(self.handle.is_none(),|ui|{ui.label(tr("ACP agent executable"));ui.add(egui::TextEdit::singleline(&mut self.program).char_limit(4096));ui.label(tr("Agent arguments (one per line; no shell parsing)"));ui.add(egui::TextEdit::multiline(&mut self.args).char_limit(32*4096).desired_rows(3));});
            ui.horizontal_wrapped(|ui|{if ui.add_enabled(trusted&&root.is_some()&&self.handle.is_none()&&!self.program.is_empty(),egui::Button::new(tr("Start ACP Agent"))).clicked(){if let Some(root)=root.clone(){self.start(root,trusted);}}if ui.add_enabled(self.handle.is_some(),egui::Button::new(tr("Stop ACP Agent"))).clicked(){self.stop();}if ui.add_enabled(self.handle.is_some()&&!self.busy&&!self.agent.is_empty(),egui::Button::new(tr("New Agent Session"))).clicked(){self.busy=true;self.command(Command::NewSession);}if ui.add_enabled(!self.running,egui::Button::new(tr("Clear ACP Display"))).clicked(){self.transcript.clear();self.tools.clear();self.logs.clear();}});
            if !self.agent.is_empty(){ui.label(format!("{}: {}",tr("ACP agent"),self.agent));}if let Some(session)=&self.session{ui.label(format!("{}: {session}",tr("Session")));}
            for auth in self.auth.clone(){ui.horizontal_wrapped(|ui|{ui.label(&auth.name);if ui.add_enabled(auth.supported&&!self.busy&&self.handle.is_some(),egui::Button::new(tr("Authenticate Agent"))).clicked(){self.busy=true;self.command(Command::Authenticate(auth.id));}if !auth.supported{ui.label(tr("Terminal agent authentication is unsupported; configure the agent externally"));}});}
            if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error);}
            for permission in self.permissions.clone(){ui.separator();ui.colored_label(rc_kit::pal(ui).warn,tr("ACP permission request: inspect the complete agent-supplied tool data before choosing"));let mut details=permission.details.clone();egui::ScrollArea::both().id_salt(("acp_permission",request_id(&permission.id).unwrap_or_default())).max_height(200.0).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut details).font(egui::TextStyle::Monospace).interactive(false).desired_width(f32::INFINITY));});for option in permission.options{ui.horizontal_wrapped(|ui|{ui.label(&option.name);let label=match option.kind.as_str(){"allow_once"=>tr("Allow this call once"),"reject_once"=>tr("Reject this call once"),_=>tr("Persistent ACP permissions are unsupported")};if ui.add_enabled(!self.cancelling&&matches!(option.kind.as_str(),"allow_once"|"reject_once"),egui::Button::new(label)).clicked(){self.command(Command::Choose{id:permission.id.clone(),option:option.id});self.permissions.retain(|pending|pending.id!=permission.id);}});}}
            ui.separator();ui.label(tr("ACP conversation"));egui::ScrollArea::vertical().id_salt("acp_conversation").max_height(260.0).stick_to_bottom(true).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut self.transcript).font(egui::TextStyle::Monospace).interactive(false).desired_rows(8));});
            ui.add_enabled_ui(!self.busy,|ui|{ui.add(egui::TextEdit::multiline(&mut self.input).char_limit(64*1024).desired_rows(3).hint_text(tr("Message to the ACP agent")));});
            ui.horizontal_wrapped(|ui|{if ui.add_enabled(trusted&&self.session.is_some()&&!self.busy&&!self.input.trim().is_empty(),egui::Button::new(tr("Send ACP Prompt"))).clicked(){let input=std::mem::take(&mut self.input);if Self::append(&mut self.transcript,&format!("\n{}: {input}\n",tr("You")),1024*1024).is_err(){self.error=Some(Error::Limit.message());}else{self.error=None;self.busy=true;self.running=true;self.command(Command::Prompt(input));}}if ui.add_enabled(self.running&&!self.cancelling,egui::Button::new(tr("Cancel ACP Prompt"))).clicked(){self.cancelling=true;self.permissions.clear();self.command(Command::CancelPrompt);}if self.busy{ui.spinner();}});
            egui::CollapsingHeader::new(tr("ACP tool updates")).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut self.tools).font(egui::TextStyle::Monospace).interactive(false).desired_rows(4));});egui::CollapsingHeader::new(tr("ACP agent log")).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut self.logs).font(egui::TextStyle::Monospace).interactive(false).desired_rows(4));});
        });
    }
}
impl Drop for Acp{fn drop(&mut self){self.stop();}}
pub fn show(app:&mut crate::app::CrabCraftApp,ui:&mut egui::Ui){if crate::ui::toolwindows::header(ui,tr("ACP Agents")){app.toggle_tool(crate::toolwindow::ToolWindow::Agents);}let root=app.workspace.as_ref().map(|workspace|workspace.ws.root.clone());let trusted=root.as_ref().is_some_and(|root|app.settings.trusted_projects.contains(root));app.acp.show(ui,root,trusted);}

#[cfg(test)]
mod tests{
    use super::*;
    #[tokio::test]
    async fn newline_framing_is_cancellation_safe_and_bounds_untrusted_input(){
        let(mut sender,receiver)=tokio::io::duplex(1024);let mut frames=Frames::new(receiver);sender.write_all(br#"{"jsonrpc":"2.0","method":"session/"#).await.unwrap();assert!(tokio::time::timeout(Duration::from_millis(10),frames.next()).await.is_err());assert!(!frames.partial.is_empty());sender.write_all(b"update\",\"params\":{}}\n").await.unwrap();assert_eq!(frames.next().await.unwrap()["method"],"session/update");
        let(mut sender,receiver)=tokio::io::duplex(FRAME+2);sender.write_all(&vec![b'x';FRAME+1]).await.unwrap();let mut frames=Frames::new(receiver);assert!(matches!(frames.next().await,Err(Error::Limit)));
        let(mut sender,receiver)=tokio::io::duplex(128);sender.write_all(b"not ACP JSON\n").await.unwrap();let mut frames=Frames::new(receiver);assert!(matches!(frames.next().await,Err(Error::Protocol)));
    }
    #[test]
    fn permission_metadata_rejects_ambiguous_ids_oversize_and_unknown_kinds(){
        let params=json!({"toolCall":{"kind":"execute","rawInput":{"command":"cargo test --offline"}},"options":[{"optionId":"yes","name":"Agent title is untrusted","kind":"allow_once"},{"optionId":"no","name":"No","kind":"reject_once"}]});let parsed=permission(json!(7),&params).unwrap();assert_eq!(parsed.options[0].kind,"allow_once");assert!(parsed.details.contains("cargo test --offline"));let mut duplicate=params.clone();duplicate["options"][1]["optionId"]=json!("yes");assert!(matches!(permission(json!(7),&duplicate),Err(Error::Protocol)));assert!(request_id(&json!(1.5)).is_err());let mut bad=params.clone();bad["options"][0]["kind"]=json!("trust_everything");assert!(permission(json!(7),&bad).is_err());bad=params.clone();bad["toolCall"]["rawInput"]=json!("x".repeat(64*1024));assert!(matches!(permission(json!(7),&bad),Err(Error::Limit)));
        let inbox=Inbox::new(Waker::noop());assert!(matches!(launch("definitely-never-run".into(),vec![],PathBuf::from("/untrusted"),false,inbox.sender()),Err(Error::Trust)));
    }
    #[cfg(unix)]
    fn agent(dir:&std::path::Path)->PathBuf{
        use std::os::unix::fs::PermissionsExt;
        let path=dir.join("fixture-acp");let script=r#"#!/usr/bin/env python3
import sys,json,pathlib,subprocess,time
root=pathlib.Path.cwd()
(root/'.agent-started').write_text('yes')
active=None
permission_id=700
mode='normal'
authed='--auth' not in sys.argv
suppress='--ignore-cancel' in sys.argv
def send(value):
 print(json.dumps(value),flush=True)
def result(i,value):send({'jsonrpc':'2.0','id':i,'result':value})
def update(text):send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'rust-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}})
for line in sys.stdin:
 msg=json.loads(line)
 with open(root/'.agent-wire','a') as f:f.write(json.dumps(msg)+'\n')
 method=msg.get('method')
 if method=='initialize':
  caps=msg['params']['clientCapabilities']
  assert caps['fs']['readTextFile'] is False and caps['fs']['writeTextFile'] is False and caps['terminal'] is False
  result(msg['id'],{'protocolVersion':2 if '--wrong-version' in sys.argv else 1,'agentInfo':{'name':'Rust fixture ACP','version':'1'},'authMethods':[{'id':'login','name':'Fixture login'}] if not authed else []})
 elif method=='authenticate':
  assert msg['params']['methodId']=='login';authed=True;result(msg['id'],{})
 elif method=='session/new':
  assert msg['params']['cwd']==str(root) and msg['params']['mcpServers']==[]
  if not authed:send({'jsonrpc':'2.0','id':msg['id'],'error':{'code':-32000,'message':'Authentication required'}})
  else:result(msg['id'],{'sessionId':'rust-session'})
 elif method=='session/prompt':
  active=msg['id'];mode=msg['params']['prompt'][0]['text']
  if mode=='unsupported-fs':send({'jsonrpc':'2.0','id':900,'method':'fs/read_text_file','params':{'sessionId':'rust-session','path':str(root/'private.txt')}})
  elif mode=='always':send({'jsonrpc':'2.0','id':permission_id,'method':'session/request_permission','params':{'sessionId':'rust-session','toolCall':{'kind':'execute','title':'Persistent fixture','rawInput':{'command':'cargo test --offline'}},'options':[{'optionId':'forever','name':'Trust forever','kind':'allow_always'}]}})
  else:send({'jsonrpc':'2.0','id':permission_id,'method':'session/request_permission','params':{'sessionId':'rust-session','toolCall':{'kind':'execute','title':'Run real Rust tests','rawInput':{'command':'cargo test --offline'},'locations':[{'path':str(root/'Cargo.toml')}]},'options':[{'optionId':'yes','name':'Fixture yes','kind':'allow_once'},{'optionId':'no','name':'Fixture no','kind':'reject_once'}]}})
 elif method=='session/cancel':
  if not suppress and active is not None:result(active,{'stopReason':'cancelled'});active=None
 elif method is None and msg.get('id')==900:
  assert msg.get('error',{}).get('code')==-32601 and 'secret-content' not in json.dumps(msg)
  update('Client file delegation refused');result(active,{'stopReason':'end_turn'});active=None
 elif method is None and msg.get('id')==permission_id:
  outcome=msg['result']['outcome']
  if outcome['outcome']=='cancelled':
   if not suppress and active is not None:result(active,{'stopReason':'cancelled'});active=None
  elif outcome.get('optionId')=='yes':
   counter=root/'.agent-cargo-count';counter.write_text(str(int(counter.read_text())+1 if counter.exists() else 1))
   output=subprocess.run(['cargo','test','--offline'],cwd=root,capture_output=True,text=True)
   assert output.returncode==0,output.stderr
   update('Real Rust tests passed');result(active,{'stopReason':'end_turn'});active=None
  else:update('Tool denied without execution');result(active,{'stopReason':'end_turn'});active=None
"#;std::fs::write(&path,script).unwrap();std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o700)).unwrap();path
    }
    #[cfg(unix)]
    fn fixture(root:&std::path::Path){std::fs::create_dir(root.join("src")).unwrap();std::fs::write(root.join("Cargo.toml"),"[package]\nname='acp-rust-proof'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();std::fs::write(root.join("src/lib.rs"),"pub fn value()->u8{42}\n#[test] fn real_rust_test(){assert_eq!(value(),42);}\n").unwrap();std::fs::write(root.join("build.rs"),"fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}\n").unwrap();std::fs::write(root.join("private.txt"),"secret-content").unwrap();}
    #[cfg(unix)]
    fn pump(client:&mut Acp,done:impl Fn(&Acp)->bool){let start=std::time::Instant::now();while start.elapsed()<Duration::from_secs(10){client.poll(true);if done(client){return;}std::thread::sleep(Duration::from_millis(10));}panic!("ACP timeout; {:?} {}",client.error,client.logs);}
    #[cfg(unix)]
    #[test]
    fn protocol_refuses_delegated_files_checks_auth_and_cancels_permissions_with_ack_or_process_stop(){
        for mode in ["normal","--auth","--ignore-cancel","--wrong-version"]{let root=tempfile::tempdir().unwrap();let root=root.path().canonicalize().unwrap();fixture(&root);let program=agent(&root);let mut client=Acp::new(Waker::noop());client.program=program.to_string_lossy().into_owned();if mode!="normal"{client.args=mode.into();}client.start(root.clone(),true);
            if mode=="--wrong-version"{pump(&mut client,|client|client.handle.is_none());assert!(client.error.as_deref()==Some(Error::Version.message().as_str()));continue;}
            if mode=="--auth"{pump(&mut client,|client|!client.busy&&!client.auth.is_empty());assert!(client.session.is_none());client.command(Command::Authenticate("login".into()));}
            pump(&mut client,|client|client.session.is_some());assert!(!root.join("build-ran").exists());
            client.command(Command::Prompt("unsupported-fs".into()));pump(&mut client,|client|client.transcript.contains("delegation refused")&&!client.running&&!client.busy);assert!(!root.join("build-ran").exists());
            client.command(Command::Prompt("always".into()));pump(&mut client,|client|!client.permissions.is_empty());let permission=client.permissions[0].clone();client.command(Command::Choose{id:permission.id,option:"forever".into()});std::thread::sleep(Duration::from_millis(30));client.poll(true);assert!(!root.join("build-ran").exists());client.command(Command::CancelPrompt);
            if mode=="--ignore-cancel"{pump(&mut client,|client|client.handle.is_none());assert!(client.error.as_deref()==Some(Error::CancelTimeout.message().as_str()));}else{pump(&mut client,|client|!client.running);assert!(client.permissions.is_empty());client.stop();pump(&mut client,|client|client.handle.is_none());}
            let wire=std::fs::read_to_string(root.join(".agent-wire")).unwrap();assert!(wire.contains("cancelled"));assert!(!wire.contains("\"optionId\": \"forever\""));assert!(!root.join("build-ran").exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn native_acp_permissions_deny_then_allow_a_real_rust_test_and_cancel_without_hidden_approval(){
        use egui_kittest::kittest::Queryable;
        let _lock=crate::screens::HOME_LOCK.lock().unwrap_or_else(|e|e.into_inner());let home=tempfile::tempdir().unwrap();let project=tempfile::tempdir().unwrap();let root=project.path().canonicalize().unwrap();fixture(&root);let program=agent(&root);let mut app=crate::screens::test_app(home.path(),true);app.open_workspace(&root);app.acp.program=program.to_string_lossy().into_owned();app.dock.sizes.insert(crate::toolwindow::ToolWindow::Agents,800.0);let mut h=crate::screens::harness(app,egui::vec2(1900.0,1500.0));
        h.get_by_label(tr("ACP Agents")).click();h.run_steps(2);h.get_by_label(tr("Start ACP Agent")).click();h.run_steps(3);assert!(!root.join(".agent-started").exists());assert!(!root.join("build-ran").exists());h.state_mut().trust_project_tools_for_test();h.run_steps(2);
        let wait=|h:&mut egui_kittest::Harness<'_,crate::app::CrabCraftApp>,test:fn(&Acp)->bool|{let start=std::time::Instant::now();while !test(&h.state().acp)&&start.elapsed()<Duration::from_secs(15){h.run_steps(2);std::thread::sleep(Duration::from_millis(10));}assert!(test(&h.state().acp),"{:?} {}",h.state().acp.error,h.state().acp.logs);};
        h.get_by_label(tr("Start ACP Agent")).click();h.run_steps(2);wait(&mut h,|client|client.session.is_some());assert_eq!(h.state().acp.agent,"Rust fixture ACP");assert!(!root.join("build-ran").exists());
        h.state_mut().acp.input="Check Rust tests before executing".into();h.get_by_label(tr("Send ACP Prompt")).click();h.run_steps(2);wait(&mut h,|client|!client.permissions.is_empty());assert!(h.state().acp.permissions[0].details.contains("cargo test --offline"));assert!(!root.join("build-ran").exists());h.get_by_label(tr("Reject this call once")).click();h.run_steps(2);wait(&mut h,|client|!client.running&&!client.busy);assert!(h.state().acp.transcript.contains("denied without execution"));assert!(!root.join(".agent-cargo-count").exists());
        h.state_mut().acp.input="Run the reviewed Rust tests".into();h.get_by_label(tr("Send ACP Prompt")).click();h.run_steps(2);wait(&mut h,|client|!client.permissions.is_empty());assert!(!root.join("build-ran").exists());h.get_by_label(tr("Allow this call once")).click();h.run_steps(2);wait(&mut h,|client|!client.running&&!client.busy);assert!(h.state().acp.transcript.contains("Real Rust tests passed"));assert!(root.join("build-ran").exists());assert_eq!(std::fs::read_to_string(root.join(".agent-cargo-count")).unwrap(),"1");
        h.state_mut().acp.input="Cancel this pending tool".into();h.get_by_label(tr("Send ACP Prompt")).click();h.run_steps(2);wait(&mut h,|client|!client.permissions.is_empty());h.get_by_label(tr("Cancel ACP Prompt")).click();h.run_steps(2);wait(&mut h,|client|!client.running&&!client.busy);assert!(h.state().acp.permissions.is_empty());assert_eq!(std::fs::read_to_string(root.join(".agent-cargo-count")).unwrap(),"1");let wire=std::fs::read_to_string(root.join(".agent-wire")).unwrap();assert!(wire.contains("session/cancel")&&wire.contains("cancelled"));h.get_by_label(tr("Stop ACP Agent")).click();h.run_steps(2);wait(&mut h,|client|client.handle.is_none());assert!(h.state().acp.session.is_none());
    }

}
