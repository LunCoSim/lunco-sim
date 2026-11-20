// @timing presentation
// Replay an equipment pick and a subject-to-subject possession before returning.
const AVATAR="/World/Avatar";
const SCOUT="/World/Scout";
const PRESS="/World/Works/PackingLine";
const TANK="/World/ProductTank1/Insulation";

fn simulation_dependencies(me,ctx) {
    #{entity_reads:[find_path(AVATAR),find_path(SCOUT),find_path(PRESS),find_path(TANK)],
      entity_writes:[find_path(AVATAR),find_path(SCOUT),find_path(PRESS)],
      modelica_entities:[],query_reads:["GetReadiness"],required_inputs:[]}
}
fn check_command(result) {if !result.ok {throw result.error;}}
fn distance(a,b) {
    let dx=a[0]-b[0];let dy=a[1]-b[1];let dz=a[2]-b[2];
    (dx*dx+dy*dy+dz*dz).sqrt()
}
fn capture_avatar() {
    this.avatar=find_path(AVATAR);
    this.position=world_pos(this.avatar);
    this.direction=world_forward(this.avatar);
    this.up=world_up(this.avatar);
    this.checks=[];
}
fn avatar_pose_restored() {
    current_camera_name()==AVATAR && distance(world_pos(this.avatar),this.position)<0.01 &&
        distance(world_forward(this.avatar),this.direction)<0.001 && distance(world_up(this.avatar),this.up)<0.001
}
fn aim_at(path) {
    this.pt=viewport_position(find_path(path));
    if this.pt==() {throw "Camera test cannot project "+path;}
    input_pointer_move(this.pt[0],this.pt[1]);
}
fn press(me) {input_pointer_press("primary",this.pt[0],this.pt[1]);}
fn release(me) {input_pointer_release("primary",this.pt[0],this.pt[1]);}
fn task(me,ctx) {
    seq([wait_until(|m| query("GetReadiness").ready && current_camera_name()==AVATAR
             && find_path(TANK)>=0 && viewport_position(find_path(TANK))!=()
             && cmd_data("InspectGuidedHud",#{}).cards.len()>0),
         once(|m| {this.capture_avatar();this.aim_at(TANK);}),wait(0.25),once(Fn("press")),wait(0.15),once(Fn("release")),wait(0.6),
         once(|m| {
             let hint=cmd_data("InspectGuidedHud",#{}).cards[0].hint;
             this.checks.push(t_true(hint.contains("Oxygen receiver") && hint.contains("kg") &&
                 hint.contains("kPa abs") && hint.contains("K"),"picking the receiver shows live gas telemetry: "+hint));
             this.aim_at(SCOUT+"/RoofPanel");
         }),wait(0.25),once(Fn("press")),wait(0.15),once(Fn("release")),
         wait_until(|m| current_camera_name().ends_with("/MastCam")),
         once(|m| {
             let driven = is_controlled(find_path(SCOUT));
             let camera = current_camera_name();
             this.checks.push(t_true(driven && camera.ends_with("/MastCam"),
                 "clicking Scout activates the mounted camera (controlled=" + driven + ", camera=" + camera + ")"));
             check_command(cmd("AcquireControl",#{target:find_path(PRESS),bind_camera:true}));
         }),wait(0.6),once(|m| {
             this.checks.push(t_true(is_controlled(find_path(PRESS)),"switching directly to the press changes possession"));
             check_command(cmd("ReleaseControlSource",#{source:this.avatar}));
         }),wait(0.6),once(|m| {
             this.checks.push(t_true(this.avatar_pose_restored(),"release restores the original avatar pose "+this.position+" -> "+world_pos(this.avatar)));
             check_command(cmd("AcquireControl",#{target:find_path(SCOUT),bind_camera:true}));
         }),wait(0.4),once(|m| {
             check_command(cmd("FocusTarget",#{target:find_path(SCOUT)}));
         }),wait(0.4),once(|m| {
             this.checks.push(t_true(is_controlled(find_path(SCOUT)) && get(this.avatar,"OrbitCamera")!=(),
                 "the avatar can enter orbit view while controlling Scout"));
             check_command(cmd("ReturnFromOrbit",#{camera:this.avatar}));
         }),wait(0.05),once(|m| {
             this.checks.push(t_true(get(this.avatar,"CameraPoseLock")!=() && get(this.avatar,"SpringArmCamera")==(),
                 "release is issued during the animated orbit return"));
             check_command(cmd("ReleaseControlSource",#{source:this.avatar}));
         }),wait(1.5),once(|m| {
             this.checks.push(t_true(!is_controlled(find_path(SCOUT)) && get(this.avatar,"CameraPoseLock")==() &&
                 get(this.avatar,"SpringArmCamera")==() && this.avatar_pose_restored(),
                 "releasing during orbit return leaves the avatar in its original pose and frees Scout"));
             report_verdict(this.checks,"CRATER CAMERA","CRATER_CAMERA");
         })])
}
