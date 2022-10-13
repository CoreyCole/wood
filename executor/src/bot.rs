use crate::{
    actor::{Actor, TargetDescriptor},
    character::Character,
    item::ItemContainer,
    level::UpdateContext,
    message::Message,
    weapon::WeaponContainer,
    GameTime,
};
use fyrox::scene::graph::physics::CoefficientCombineRule;
use fyrox::utils::navmesh::NavmeshAgent;
use fyrox::{
    animation::{
        machine::{self, Machine, PoseNode, State},
        Animation, AnimationSignal,
    },
    core::{
        algebra::{Matrix4, Point3, UnitQuaternion, Vector3},
        color::Color,
        math::{frustum::Frustum, ray::Ray, SmoothAngle, Vector3Ext},
        pool::Handle,
        rand::Rng,
        visitor::{Visit, VisitResult, Visitor},
    },
    engine::resource_manager::ResourceManager,
    rand,
    resource::model::Model,
    scene::{
        self,
        base::BaseBuilder,
        collider::{ColliderBuilder, ColliderShape, InteractionGroups},
        debug::SceneDrawingContext,
        graph::{physics::RayCastOptions, Graph},
        node::Node,
        pivot::PivotBuilder,
        rigidbody::RigidBodyBuilder,
        transform::TransformBuilder,
        Scene,
    },
    utils::log::{Log, MessageKind},
};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    sync::mpsc::Sender,
};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Visit)]
pub enum BotKind {
    // Beasts
    Mutant,
    Parasite,
    Maw,
    // Humans
}

impl Default for BotKind {
    fn default() -> Self {
        Self::Mutant
    }
}

impl BotKind {
    pub fn description(self) -> &'static str {
        match self {
            BotKind::Mutant => "Mutant",
            BotKind::Parasite => "Parasite",
            BotKind::Maw => "Maw",
        }
    }
}

#[derive(Debug, Default, Visit)]
pub struct Target {
    position: Vector3<f32>,
    handle: Handle<Actor>,
}

#[derive(Visit)]
pub struct Bot {
    target: Option<Target>,
    kind: BotKind,
    model: Handle<Node>,
    character: Character,
    locomotion_machine: LocomotionMachine,
    combat_machine: CombatMachine,
    dying_machine: DyingMachine,
    last_health: f32,
    restoration_time: f32,
    #[visit(skip)]
    navmesh_agent: NavmeshAgent,
    frustum: Frustum,
    last_poi_update_time: f64,
    point_of_interest: Vector3<f32>,
    last_move_dir: Vector3<f32>,
    spine: Handle<Node>,
    yaw: SmoothAngle,
    pitch: SmoothAngle,
}

impl Deref for Bot {
    type Target = Character;

    fn deref(&self) -> &Self::Target {
        &self.character
    }
}

impl DerefMut for Bot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.character
    }
}

impl Default for Bot {
    fn default() -> Self {
        Self {
            character: Default::default(),
            kind: BotKind::Mutant,
            model: Default::default(),
            target: Default::default(),
            locomotion_machine: Default::default(),
            combat_machine: Default::default(),
            dying_machine: Default::default(),
            last_health: 0.0,
            restoration_time: 0.0,
            navmesh_agent: Default::default(),
            frustum: Default::default(),
            last_poi_update_time: -10.0,
            point_of_interest: Default::default(),
            last_move_dir: Default::default(),
            spine: Default::default(),
            yaw: SmoothAngle {
                angle: 0.0,
                target: 0.0,
                speed: 260.0f32.to_radians(), // rad/s
            },
            pitch: SmoothAngle {
                angle: 0.0,
                target: 0.0,
                speed: 260.0f32.to_radians(), // rad/s
            },
        }
    }
}

pub struct BotDefinition {
    pub scale: f32,
    pub health: f32,
    pub kind: BotKind,
    pub walk_speed: f32,
    pub weapon_scale: f32,
    pub model: &'static str,
    pub idle_animation: &'static str,
    pub walk_animation: &'static str,
    pub aim_animation: &'static str,
    pub whip_animation: &'static str,
    pub jump_animation: &'static str,
    pub falling_animation: &'static str,
    pub hit_reaction_animation: &'static str,
    pub dying_animation: &'static str,
    pub dead_animation: &'static str,
    pub weapon_hand_name: &'static str,
    pub left_leg_name: &'static str,
    pub right_leg_name: &'static str,
    pub spine: &'static str,
    pub v_aim_angle_hack: f32,
}

fn prepare_animation(
    scene: &mut Scene,
    model: Model,
    root: Handle<Node>,
    spine: Handle<Node>,
) -> Handle<Animation> {
    let animation = model.retarget_animations(root, scene)[0];
    scene
        .animations
        .get_mut(animation)
        .set_node_track_enabled(spine, false);
    animation
}

fn disable_leg_tracks(
    animation: &mut Animation,
    root: Handle<Node>,
    leg_name: &str,
    graph: &Graph,
) {
    animation.set_tracks_enabled_from(graph.find_by_name(root, leg_name), false, graph)
}

#[derive(Visit)]
struct LocomotionMachine {
    machine: Machine,
    walk_animation: Handle<Animation>,
    walk_state: Handle<State>,
}

impl Default for LocomotionMachine {
    fn default() -> Self {
        Self {
            machine: Default::default(),
            walk_animation: Default::default(),
            walk_state: Default::default(),
        }
    }
}

impl LocomotionMachine {
    pub const STEP_SIGNAL: u64 = 1;

    const WALK_TO_IDLE_PARAM: &'static str = "WalkToIdle";
    const WALK_TO_JUMP_PARAM: &'static str = "WalkToJump";
    const IDLE_TO_WALK_PARAM: &'static str = "IdleToWalk";
    const IDLE_TO_JUMP_PARAM: &'static str = "IdleToJump";
    const JUMP_TO_FALLING_PARAM: &'static str = "JumpToFalling";
    const FALLING_TO_IDLE_PARAM: &'static str = "FallingToIdle";

    async fn new(
        resource_manager: ResourceManager,
        definition: &BotDefinition,
        model: Handle<Node>,
        scene: &mut Scene,
        spine: Handle<Node>,
    ) -> Self {
        let (idle_animation, walk_animation, jump_animation, falling_animation) = fyrox::core::futures::join!(
            resource_manager.request_model(definition.idle_animation,),
            resource_manager.request_model(definition.walk_animation,),
            resource_manager.request_model(definition.jump_animation,),
            resource_manager.request_model(definition.falling_animation,)
        );

        let idle_animation = prepare_animation(scene, idle_animation.unwrap(), model, spine);
        let walk_animation = prepare_animation(scene, walk_animation.unwrap(), model, spine);
        let jump_animation = prepare_animation(scene, jump_animation.unwrap(), model, spine);
        let falling_animation = prepare_animation(scene, falling_animation.unwrap(), model, spine);

        scene
            .animations
            .get_mut(walk_animation)
            .add_signal(AnimationSignal::new(Self::STEP_SIGNAL, 0.4))
            .add_signal(AnimationSignal::new(Self::STEP_SIGNAL, 0.8));

        let mut machine = Machine::new(model);

        let jump_node = machine.add_node(machine::PoseNode::make_play_animation(jump_animation));
        let jump_state = machine.add_state(State::new("Jump", jump_node));

        let falling_node =
            machine.add_node(machine::PoseNode::make_play_animation(falling_animation));
        let falling_state = machine.add_state(State::new("Falling", falling_node));

        let walk_node = machine.add_node(machine::PoseNode::make_play_animation(walk_animation));
        let walk_state = machine.add_state(State::new("Walk", walk_node));

        let idle_node = machine.add_node(machine::PoseNode::make_play_animation(idle_animation));
        let idle_state = machine.add_state(State::new("Idle", idle_node));

        machine.add_transition(machine::Transition::new(
            "Walk->Idle",
            walk_state,
            idle_state,
            0.5,
            Self::WALK_TO_IDLE_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Walk->Jump",
            walk_state,
            jump_state,
            0.5,
            Self::WALK_TO_JUMP_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Idle->Walk",
            idle_state,
            walk_state,
            0.5,
            Self::IDLE_TO_WALK_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Idle->Jump",
            idle_state,
            jump_state,
            0.5,
            Self::IDLE_TO_JUMP_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Jump->Falling",
            jump_state,
            falling_state,
            0.5,
            Self::JUMP_TO_FALLING_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Falling->Idle",
            falling_state,
            idle_state,
            0.5,
            Self::FALLING_TO_IDLE_PARAM,
        ));

        machine.set_entry_state(idle_state);

        Self {
            walk_animation,
            walk_state,
            machine,
        }
    }

    fn is_walking(&self) -> bool {
        let active_transition = self.machine.active_transition();
        self.machine.active_state() == self.walk_state
            || (active_transition.is_some()
                && self.machine.transitions().borrow(active_transition).dest() == self.walk_state)
    }

    fn clean_up(&mut self, scene: &mut Scene) {
        clean_machine(&self.machine, scene);
    }

    fn apply(
        &mut self,
        scene: &mut Scene,
        time: GameTime,
        in_close_combat: bool,
        need_jump: bool,
        has_ground_contact: bool,
    ) {
        self.machine
            .set_parameter(
                Self::IDLE_TO_WALK_PARAM,
                machine::Parameter::Rule(!in_close_combat),
            )
            .set_parameter(
                Self::WALK_TO_IDLE_PARAM,
                machine::Parameter::Rule(in_close_combat),
            )
            .set_parameter(
                Self::WALK_TO_JUMP_PARAM,
                machine::Parameter::Rule(need_jump),
            )
            .set_parameter(
                Self::IDLE_TO_JUMP_PARAM,
                machine::Parameter::Rule(need_jump),
            )
            .set_parameter(
                Self::JUMP_TO_FALLING_PARAM,
                machine::Parameter::Rule(!has_ground_contact),
            )
            .set_parameter(
                Self::FALLING_TO_IDLE_PARAM,
                machine::Parameter::Rule(has_ground_contact),
            )
            .evaluate_pose(&scene.animations, time.delta)
            .apply(&mut scene.graph);
    }
}

#[derive(Visit)]
struct DyingMachine {
    machine: Machine,
    dead_state: Handle<State>,
    dead_animation: Handle<Animation>,
    dying_animation: Handle<Animation>,
}

impl Default for DyingMachine {
    fn default() -> Self {
        Self {
            machine: Default::default(),
            dead_state: Default::default(),
            dead_animation: Default::default(),
            dying_animation: Default::default(),
        }
    }
}

impl DyingMachine {
    const DYING_TO_DEAD: &'static str = "DyingToDead";

    async fn new(
        resource_manager: ResourceManager,
        definition: &BotDefinition,
        model: Handle<Node>,
        scene: &mut Scene,
        spine: Handle<Node>,
    ) -> Self {
        let (dying_animation, dead_animation) = fyrox::core::futures::join!(
            resource_manager.request_model(definition.dying_animation,),
            resource_manager.request_model(definition.dead_animation,)
        );

        let dying_animation = prepare_animation(scene, dying_animation.unwrap(), model, spine);
        let dead_animation = prepare_animation(scene, dead_animation.unwrap(), model, spine);

        scene
            .animations
            .get_mut(dying_animation)
            .set_enabled(false)
            .set_speed(1.5);

        scene
            .animations
            .get_mut(dead_animation)
            .set_enabled(false)
            .set_loop(false);

        let mut machine = Machine::new(model);

        let dying_node = machine.add_node(machine::PoseNode::make_play_animation(dying_animation));
        let dying_state = machine.add_state(State::new("Dying", dying_node));

        let dead_node = machine.add_node(machine::PoseNode::make_play_animation(dead_animation));
        let dead_state = machine.add_state(State::new("Dead", dead_node));

        machine.set_entry_state(dying_state);

        machine.add_transition(machine::Transition::new(
            "Dying->Dead",
            dying_state,
            dead_state,
            1.5,
            Self::DYING_TO_DEAD,
        ));

        Self {
            machine,
            dead_state,
            dead_animation,
            dying_animation,
        }
    }

    fn clean_up(&mut self, scene: &mut Scene) {
        clean_machine(&self.machine, scene);
    }

    fn apply(&mut self, scene: &mut Scene, time: GameTime, is_dead: bool) {
        scene
            .animations
            .get_mut(self.dying_animation)
            .set_enabled(true);
        scene
            .animations
            .get_mut(self.dead_animation)
            .set_enabled(true);

        self.machine
            .set_parameter(Self::DYING_TO_DEAD, machine::Parameter::Rule(is_dead))
            .evaluate_pose(&scene.animations, time.delta)
            .apply(&mut scene.graph);
    }
}

#[derive(Visit)]
struct CombatMachine {
    machine: Machine,
    hit_reaction_animation: Handle<Animation>,
    whip_animation: Handle<Animation>,
    aim_state: Handle<State>,
}

impl Default for CombatMachine {
    fn default() -> Self {
        Self {
            machine: Default::default(),
            hit_reaction_animation: Default::default(),
            whip_animation: Default::default(),
            aim_state: Default::default(),
        }
    }
}

impl CombatMachine {
    pub const HIT_SIGNAL: u64 = 1;

    const AIM_TO_WHIP_PARAM: &'static str = "AimToWhip";
    const WHIP_TO_AIM_PARAM: &'static str = "WhipToAim";
    const HIT_REACTION_TO_AIM_PARAM: &'static str = "HitReactionToAim";
    const AIM_TO_HIT_REACTION_PARAM: &'static str = "AimToHitReaction";
    const WHIP_TO_HIT_REACTION_PARAM: &'static str = "WhipToHitReaction";

    async fn new(
        resource_manager: ResourceManager,
        definition: &BotDefinition,
        model: Handle<Node>,
        scene: &mut Scene,
        spine: Handle<Node>,
    ) -> Self {
        let (aim_animation, whip_animation, hit_reaction_animation) = fyrox::core::futures::join!(
            resource_manager.request_model(definition.aim_animation,),
            resource_manager.request_model(definition.whip_animation,),
            resource_manager.request_model(definition.hit_reaction_animation,)
        );

        let aim_animation = prepare_animation(scene, aim_animation.unwrap(), model, spine);
        let whip_animation = prepare_animation(scene, whip_animation.unwrap(), model, spine);
        let hit_reaction_animation =
            prepare_animation(scene, hit_reaction_animation.unwrap(), model, spine);

        scene
            .animations
            .get_mut(whip_animation)
            .add_signal(AnimationSignal::new(Self::HIT_SIGNAL, 0.9));

        scene
            .animations
            .get_mut(hit_reaction_animation)
            .set_loop(false)
            .set_speed(2.0);

        // These animations must *not* affect legs, because legs animated using locomotion machine
        disable_leg_tracks(
            scene.animations.get_mut(aim_animation),
            model,
            definition.left_leg_name,
            &scene.graph,
        );
        disable_leg_tracks(
            scene.animations.get_mut(aim_animation),
            model,
            definition.right_leg_name,
            &scene.graph,
        );

        disable_leg_tracks(
            scene.animations.get_mut(whip_animation),
            model,
            definition.left_leg_name,
            &scene.graph,
        );
        disable_leg_tracks(
            scene.animations.get_mut(whip_animation),
            model,
            definition.right_leg_name,
            &scene.graph,
        );

        disable_leg_tracks(
            scene.animations.get_mut(hit_reaction_animation),
            model,
            definition.left_leg_name,
            &scene.graph,
        );
        disable_leg_tracks(
            scene.animations.get_mut(hit_reaction_animation),
            model,
            definition.right_leg_name,
            &scene.graph,
        );

        let mut machine = Machine::new(model);

        let hit_reaction_node = machine.add_node(machine::PoseNode::make_play_animation(
            hit_reaction_animation,
        ));
        let hit_reaction_state = machine.add_state(State::new("HitReaction", hit_reaction_node));

        let aim_node = machine.add_node(machine::PoseNode::make_play_animation(aim_animation));
        let aim_state = machine.add_state(State::new("Aim", aim_node));

        let whip_node = machine.add_node(machine::PoseNode::make_play_animation(whip_animation));
        let whip_state = machine.add_state(State::new("Whip", whip_node));

        machine.add_transition(machine::Transition::new(
            "Aim->Whip",
            aim_state,
            whip_state,
            0.5,
            Self::AIM_TO_WHIP_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Whip->Aim",
            whip_state,
            aim_state,
            0.5,
            Self::WHIP_TO_AIM_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Whip->HitReaction",
            whip_state,
            hit_reaction_state,
            0.2,
            Self::WHIP_TO_HIT_REACTION_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "Aim->HitReaction",
            aim_state,
            hit_reaction_state,
            0.2,
            Self::AIM_TO_HIT_REACTION_PARAM,
        ));
        machine.add_transition(machine::Transition::new(
            "HitReaction->Aim",
            hit_reaction_state,
            aim_state,
            0.5,
            Self::HIT_REACTION_TO_AIM_PARAM,
        ));

        Self {
            machine,
            hit_reaction_animation,
            whip_animation,
            aim_state,
        }
    }

    fn clean_up(&mut self, scene: &mut Scene) {
        clean_machine(&self.machine, scene)
    }

    fn apply(
        &mut self,
        scene: &mut Scene,
        time: GameTime,
        in_close_combat: bool,
        was_damaged: bool,
        can_aim: bool,
    ) {
        self.machine
            .set_parameter(
                Self::WHIP_TO_AIM_PARAM,
                machine::Parameter::Rule(!in_close_combat),
            )
            .set_parameter(
                Self::AIM_TO_WHIP_PARAM,
                machine::Parameter::Rule(in_close_combat),
            )
            .set_parameter(
                Self::WHIP_TO_HIT_REACTION_PARAM,
                machine::Parameter::Rule(was_damaged),
            )
            .set_parameter(
                Self::AIM_TO_HIT_REACTION_PARAM,
                machine::Parameter::Rule(was_damaged),
            )
            .set_parameter(
                Self::HIT_REACTION_TO_AIM_PARAM,
                machine::Parameter::Rule(can_aim),
            )
            .evaluate_pose(&scene.animations, time.delta)
            .apply(&mut scene.graph);
    }
}

impl Bot {
    pub fn get_definition(kind: BotKind) -> &'static BotDefinition {
        match kind {
            BotKind::Mutant => {
                static DEFINITION: BotDefinition = BotDefinition {
                    kind: BotKind::Mutant,
                    model: "data/models/mutant.FBX",
                    idle_animation: "data/animations/mutant/idle.fbx",
                    walk_animation: "data/animations/mutant/walk.fbx",
                    aim_animation: "data/animations/mutant/aim.fbx",
                    whip_animation: "data/animations/mutant/whip.fbx",
                    jump_animation: "data/animations/mutant/jump.fbx",
                    falling_animation: "data/animations/mutant/falling.fbx",
                    dying_animation: "data/animations/mutant/dying.fbx",
                    dead_animation: "data/animations/mutant/dead.fbx",
                    hit_reaction_animation: "data/animations/mutant/hit_reaction.fbx",
                    weapon_hand_name: "Mutant:RightHand",
                    left_leg_name: "Mutant:LeftUpLeg",
                    right_leg_name: "Mutant:RightUpLeg",
                    spine: "Mutant:Spine",
                    walk_speed: 2.0,
                    scale: 0.0085,
                    weapon_scale: 2.6,
                    health: 100.0,
                    v_aim_angle_hack: -2.0,
                };
                &DEFINITION
            }
            BotKind::Parasite => {
                static DEFINITION: BotDefinition = BotDefinition {
                    kind: BotKind::Parasite,
                    model: "data/models/parasite.FBX",
                    idle_animation: "data/animations/parasite/idle.fbx",
                    walk_animation: "data/animations/parasite/walk.fbx",
                    aim_animation: "data/animations/parasite/aim.fbx",
                    whip_animation: "data/animations/parasite/whip.fbx",
                    jump_animation: "data/animations/parasite/jump.fbx",
                    falling_animation: "data/animations/parasite/falling.fbx",
                    dying_animation: "data/animations/parasite/dying.fbx",
                    dead_animation: "data/animations/parasite/dead.fbx",
                    hit_reaction_animation: "data/animations/parasite/hit_reaction.fbx",
                    weapon_hand_name: "RightHand",
                    left_leg_name: "LeftUpLeg",
                    right_leg_name: "RightUpLeg",
                    spine: "Spine",
                    walk_speed: 2.0,
                    scale: 0.0085,
                    weapon_scale: 2.5,
                    health: 100.0,
                    v_aim_angle_hack: 12.0,
                };
                &DEFINITION
            }
            BotKind::Maw => {
                static DEFINITION: BotDefinition = BotDefinition {
                    kind: BotKind::Maw,
                    model: "data/models/maw.fbx",
                    idle_animation: "data/animations/maw/idle.fbx",
                    walk_animation: "data/animations/maw/walk.fbx",
                    aim_animation: "data/animations/maw/aim.fbx",
                    whip_animation: "data/animations/maw/whip.fbx",
                    jump_animation: "data/animations/maw/jump.fbx",
                    falling_animation: "data/animations/maw/falling.fbx",
                    dying_animation: "data/animations/maw/dying.fbx",
                    dead_animation: "data/animations/maw/dead.fbx",
                    hit_reaction_animation: "data/animations/maw/hit_reaction.fbx",
                    weapon_hand_name: "RightHand",
                    left_leg_name: "LeftUpLeg",
                    right_leg_name: "RightUpLeg",
                    spine: "Spine",
                    walk_speed: 2.0,
                    scale: 0.0085,
                    weapon_scale: 2.5,
                    health: 100.0,
                    v_aim_angle_hack: 16.0,
                };
                &DEFINITION
            }
        }
    }

    pub async fn new(
        kind: BotKind,
        resource_manager: ResourceManager,
        scene: &mut Scene,
        position: Vector3<f32>,
        sender: Sender<Message>,
    ) -> Self {
        let definition = Self::get_definition(kind);

        let body_height = 1.25;

        let model = resource_manager
            .request_model(Path::new(definition.model))
            .await
            .unwrap()
            .instantiate_geometry(scene);

        scene.graph[model]
            .local_transform_mut()
            .set_position(Vector3::new(0.0, -body_height * 0.5, 0.0))
            .set_scale(Vector3::new(
                definition.scale,
                definition.scale,
                definition.scale,
            ));

        let spine = scene.graph.find_by_name(model, definition.spine);
        if spine.is_none() {
            Log::writeln(
                MessageKind::Warning,
                "Spine bone not found, bot won't aim vertically!".to_owned(),
            );
        }

        let collider;
        let body = RigidBodyBuilder::new(
            BaseBuilder::new()
                .with_local_transform(
                    TransformBuilder::new()
                        .with_local_position(position)
                        .build(),
                )
                .with_children(&[
                    {
                        collider = ColliderBuilder::new(BaseBuilder::new())
                            .with_shape(ColliderShape::capsule_y(body_height * 0.5, 0.28))
                            .with_friction_combine_rule(CoefficientCombineRule::Min)
                            .build(&mut scene.graph);
                        collider
                    },
                    model,
                ]),
        )
        .with_locked_rotations(true)
        .with_can_sleep(false)
        .build(&mut scene.graph);

        let hand = scene.graph.find_by_name(model, definition.weapon_hand_name);
        let wpn_scale = definition.weapon_scale * (1.0 / definition.scale);
        let weapon_pivot = PivotBuilder::new(
            BaseBuilder::new().with_local_transform(
                TransformBuilder::new()
                    .with_local_scale(Vector3::new(wpn_scale, wpn_scale, wpn_scale))
                    .with_local_rotation(
                        UnitQuaternion::from_axis_angle(
                            &Vector3::x_axis(),
                            std::f32::consts::FRAC_PI_2,
                        ) * UnitQuaternion::from_axis_angle(
                            &Vector3::y_axis(),
                            -std::f32::consts::FRAC_PI_2,
                        ),
                    )
                    .build(),
            ),
        )
        .build(&mut scene.graph);

        scene.graph.link_nodes(weapon_pivot, hand);

        let locomotion_machine =
            LocomotionMachine::new(resource_manager.clone(), &definition, model, scene, spine)
                .await;
        let combat_machine =
            CombatMachine::new(resource_manager.clone(), definition, model, scene, spine).await;
        let dying_machine =
            DyingMachine::new(resource_manager, definition, model, scene, spine).await;

        Self {
            character: Character {
                body,
                collider,
                weapon_pivot,
                health: definition.health,
                sender: Some(sender),
                name: format!("{:?}", kind),
                ..Default::default()
            },
            spine,
            last_health: definition.health,
            model,
            kind,
            locomotion_machine,
            combat_machine,
            dying_machine,
            ..Default::default()
        }
    }

    pub fn can_be_removed(&self) -> bool {
        self.dying_machine.machine.active_state() == self.dying_machine.dead_state
    }

    pub fn can_shoot(&self) -> bool {
        self.combat_machine.machine.active_state() == self.combat_machine.aim_state
    }

    fn select_target(
        &mut self,
        self_handle: Handle<Actor>,
        scene: &mut Scene,
        targets: &[TargetDescriptor],
    ) {
        self.target = None;
        let position = self.character.position(&scene.graph);
        let mut closest_distance = std::f32::MAX;

        let mut query_buffer = Vec::default();
        'target_loop: for desc in targets {
            if desc.handle != self_handle && self.frustum.is_contains_point(desc.position) {
                let ray = Ray::from_two_points(desc.position, position);
                scene.graph.physics.cast_ray(
                    RayCastOptions {
                        ray_origin: Point3::from(ray.origin),
                        ray_direction: ray.dir,
                        groups: InteractionGroups::default(),
                        max_len: ray.dir.norm(),
                        sort_results: true,
                    },
                    &mut query_buffer,
                );

                'hit_loop: for hit in query_buffer.iter() {
                    let collider = scene.graph[hit.collider].as_collider();
                    let body = collider.parent();

                    if matches!(collider.shape(), ColliderShape::Trimesh(_)) {
                        // Target is behind something.
                        continue 'target_loop;
                    } else {
                        // Prevent setting self as target.
                        if self.character.body == body {
                            continue 'hit_loop;
                        }
                    }
                }

                let sqr_d = position.sqr_distance(&desc.position);
                if sqr_d < closest_distance {
                    self.target = Some(Target {
                        position: desc.position,
                        handle: desc.handle,
                    });
                    closest_distance = sqr_d;
                }
            }
        }
    }

    fn select_point_of_interest(&mut self, items: &ItemContainer, scene: &Scene, time: &GameTime) {
        if time.elapsed - self.last_poi_update_time >= 1.25 {
            // Select closest non-despawned item as point of interest.
            let self_position = self.position(&scene.graph);
            let mut closest_distance = std::f32::MAX;
            for item in items.iter() {
                if !item.is_picked_up() {
                    let item_position = item.position(&scene.graph);
                    let sqr_d = item_position.sqr_distance(&self_position);
                    if sqr_d < closest_distance {
                        closest_distance = sqr_d;
                        self.point_of_interest = item_position;
                    }
                }
            }
            self.last_poi_update_time = time.elapsed;
        }
    }

    fn select_weapon(&mut self, weapons: &WeaponContainer) {
        if self.character.current_weapon().is_some()
            && weapons[self.character.current_weapon()].ammo() == 0
        {
            for (i, handle) in self.character.weapons().iter().enumerate() {
                if weapons[*handle].ammo() > 0 {
                    self.character.set_current_weapon(i);
                    break;
                }
            }
        }
    }

    pub fn debug_draw(&self, context: &mut SceneDrawingContext) {
        for pts in self.navmesh_agent.path().windows(2) {
            let a = pts[0];
            let b = pts[1];
            context.add_line(scene::debug::Line {
                begin: a,
                end: b,
                color: Color::RED,
            });
        }

        context.draw_sphere(self.navmesh_agent.position(), 10, 10, 0.25, Color::RED);

        context.draw_frustum(&self.frustum, Color::from_rgba(0, 200, 0, 255));
    }

    fn update_frustum(&mut self, position: Vector3<f32>, graph: &Graph) {
        let head_pos = position + Vector3::new(0.0, 0.8, 0.0);
        let up = graph[self.model].up_vector();
        let look_at = head_pos + graph[self.model].look_vector();
        let view_matrix = Matrix4::look_at_rh(&Point3::from(head_pos), &Point3::from(look_at), &up);
        let projection_matrix =
            Matrix4::new_perspective(16.0 / 9.0, 90.0f32.to_radians(), 0.1, 20.0);
        let view_projection_matrix = projection_matrix * view_matrix;
        self.frustum = Frustum::from(view_projection_matrix).unwrap();
    }

    pub fn definition(&self) -> &BotDefinition {
        Self::get_definition(self.kind)
    }

    fn aim_vertically(&mut self, look_dir: Vector3<f32>, graph: &mut Graph, time: GameTime) {
        let angle = self.pitch.angle();
        self.pitch
            .set_target(
                look_dir.dot(&Vector3::y()).acos() - std::f32::consts::PI / 2.0
                    + self.definition().v_aim_angle_hack.to_radians(),
            )
            .update(time.delta);

        if self.spine.is_some() {
            graph[self.spine]
                .local_transform_mut()
                .set_rotation(UnitQuaternion::from_axis_angle(&Vector3::x_axis(), angle));
        }
    }

    fn aim_horizontally(&mut self, look_dir: Vector3<f32>, graph: &mut Graph, time: GameTime) {
        let angle = self.yaw.angle();
        self.yaw
            .set_target(look_dir.x.atan2(look_dir.z))
            .update(time.delta);

        graph[self.body]
            .local_transform_mut()
            .set_rotation(UnitQuaternion::from_axis_angle(&Vector3::y_axis(), angle));
    }

    pub fn update(
        &mut self,
        self_handle: Handle<Actor>,
        context: &mut UpdateContext,
        targets: &[TargetDescriptor],
    ) {
        if self.character.is_dead() {
            self.dying_machine
                .apply(context.scene, context.time, self.character.is_dead());

            // Lock dead bot in-place.
            let body = context.scene.graph[self.body].as_rigid_body_mut();
            let mut lin_vel = body.lin_vel();
            lin_vel.x = 0.0;
            lin_vel.z = 0.0;
            body.set_lin_vel(lin_vel);
        } else {
            self.select_target(self_handle, context.scene, targets);
            self.select_weapon(context.weapons);
            self.select_point_of_interest(context.items, context.scene, &context.time);

            let has_ground_contact = self.character.has_ground_contact(&context.scene.graph);
            let body = context.scene.graph[self.character.body].as_rigid_body_mut();
            let (in_close_combat, look_dir) = match self.target.as_ref() {
                None => (false, self.point_of_interest - body.global_position()),
                Some(target) => {
                    let d = target.position - body.global_position();
                    let close_combat_threshold = 2.0;
                    (d.norm() <= close_combat_threshold, d)
                }
            };

            let position = body.global_position();

            if let Some(navmesh) = context.scene.navmeshes.at_mut(0) {
                self.navmesh_agent
                    .set_position(position - Vector3::new(0.0, 0.45, 0.0));
                if let Some(target) = self.target.as_ref() {
                    self.navmesh_agent.set_target(target.position);
                } else {
                    self.navmesh_agent.set_target(self.point_of_interest);
                }
                self.navmesh_agent.set_speed(self.definition().walk_speed);
                let _ = self.navmesh_agent.update(context.time.delta, navmesh);
            }

            let need_jump = look_dir.y >= 0.3 && has_ground_contact && in_close_combat;
            if need_jump {
                body.set_lin_vel(Vector3::new(body.lin_vel().x, 0.08, body.lin_vel().z));
            }
            let was_damaged = self.character.health < self.last_health;
            if was_damaged {
                let hit_reaction = context
                    .scene
                    .animations
                    .get_mut(self.combat_machine.hit_reaction_animation);
                if hit_reaction.has_ended() {
                    hit_reaction.rewind();
                }
                self.restoration_time = 0.8;
            }
            let can_aim = self.restoration_time <= 0.0;
            self.last_health = self.character.health;

            if !in_close_combat {
                if has_ground_contact {
                    if let Some(move_dir) =
                        (self.navmesh_agent.position() - position).try_normalize(f32::EPSILON)
                    {
                        let mut vel = move_dir.scale(1.0 / context.time.delta);
                        vel.y = body.lin_vel().y;
                        body.set_lin_vel(vel);
                        self.last_move_dir = move_dir;
                    }
                } else {
                    // A bit of air control. This helps jump of ledges when there is jump pad below bot.
                    let mut vel = self
                        .last_move_dir
                        .scale(self.definition().walk_speed / context.time.delta);
                    vel.y = body.lin_vel().y;
                    body.set_lin_vel(vel);
                }
            }

            self.update_frustum(position, &context.scene.graph);

            if let Some(look_dir) = look_dir.try_normalize(f32::EPSILON) {
                self.aim_vertically(look_dir, &mut context.scene.graph, context.time);
                self.aim_horizontally(look_dir, &mut context.scene.graph, context.time);
            }

            self.locomotion_machine.apply(
                context.scene,
                context.time,
                in_close_combat,
                need_jump,
                has_ground_contact,
            );
            self.combat_machine.apply(
                context.scene,
                context.time,
                in_close_combat,
                was_damaged,
                can_aim,
            );

            let sender = self.character.sender.as_ref().unwrap();

            if !in_close_combat && can_aim && self.can_shoot() && self.target.is_some() {
                if let Some(weapon) = self
                    .character
                    .weapons
                    .get(self.character.current_weapon as usize)
                {
                    sender
                        .send(Message::ShootWeapon {
                            weapon: *weapon,
                            initial_velocity: Vector3::default(),
                            direction: Some(look_dir),
                        })
                        .unwrap();
                }
            }

            // Apply damage to target from melee attack
            if let Some(target) = self.target.as_ref() {
                while let Some(event) = context
                    .scene
                    .animations
                    .get_mut(self.combat_machine.whip_animation)
                    .pop_event()
                {
                    if event.signal_id == CombatMachine::HIT_SIGNAL && in_close_combat {
                        sender
                            .send(Message::DamageActor {
                                actor: target.handle,
                                who: Default::default(),
                                amount: 20.0,
                            })
                            .unwrap();
                    }
                }
            }

            // Emit step sounds from walking animation.
            if self.locomotion_machine.is_walking() {
                while let Some(event) = context
                    .scene
                    .animations
                    .get_mut(self.locomotion_machine.walk_animation)
                    .pop_event()
                {
                    if event.signal_id == LocomotionMachine::STEP_SIGNAL && has_ground_contact {
                        let footsteps = [
                            "data/sounds/footsteps/FootStep_shoe_stone_step1.wav",
                            "data/sounds/footsteps/FootStep_shoe_stone_step2.wav",
                            "data/sounds/footsteps/FootStep_shoe_stone_step3.wav",
                            "data/sounds/footsteps/FootStep_shoe_stone_step4.wav",
                        ];
                        sender
                            .send(Message::PlaySound {
                                path: footsteps[rand::thread_rng().gen_range(0..footsteps.len())]
                                    .into(),
                                position,
                                gain: 1.0,
                                rolloff_factor: 2.0,
                                radius: 3.0,
                            })
                            .unwrap();
                    }
                }
            }

            self.restoration_time -= context.time.delta;
        }
    }

    pub fn clean_up(&mut self, scene: &mut Scene) {
        self.combat_machine.clean_up(scene);
        self.dying_machine.clean_up(scene);
        self.locomotion_machine.clean_up(scene);
        self.character.clean_up(scene);
    }

    pub fn on_actor_removed(&mut self, handle: Handle<Actor>) {
        if let Some(target) = self.target.as_ref() {
            if target.handle == handle {
                self.target = None;
            }
        }
    }

    pub fn set_point_of_interest(&mut self, poi: Vector3<f32>, time: GameTime) {
        self.point_of_interest = poi;
        self.last_poi_update_time = time.elapsed;
    }
}

fn clean_machine(machine: &Machine, scene: &mut Scene) {
    for node in machine.nodes() {
        if let PoseNode::PlayAnimation(node) = node {
            scene.animations.remove(node.animation);
        }
    }
}
