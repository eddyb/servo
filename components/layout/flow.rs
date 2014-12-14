/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Servo's experimental layout system builds a tree of `Flow` and `Fragment` objects and solves
//! layout constraints to obtain positions and display attributes of tree nodes. Positions are
//! computed in several tree traversals driven by the fundamental data dependencies required by
/// inline and block layout.
///
/// Flows are interior nodes in the layout tree and correspond closely to *flow contexts* in the
/// CSS specification. Flows are responsible for positioning their child flow contexts and
/// fragments. Flows have purpose-specific fields, such as auxiliary line structs, out-of-flow
/// child lists, and so on.
///
/// Currently, the important types of flows are:
///
/// * `BlockFlow`: A flow that establishes a block context. It has several child flows, each of
///   which are positioned according to block formatting context rules (CSS block boxes). Block
///   flows also contain a single box to represent their rendered borders, padding, etc.
///   The BlockFlow at the root of the tree has special behavior: it stretches to the boundaries of
///   the viewport.
///
/// * `InlineFlow`: A flow that establishes an inline context. It has a flat list of child
///   fragments/flows that are subject to inline layout and line breaking and structs to represent
///   line breaks and mapping to CSS boxes, for the purpose of handling `getClientRects()` and
///   similar methods.

use css::node_style::StyledNode;
use block::BlockFlow;
use context::LayoutContext;
use display_list_builder::{DisplayListBuildingResult, DisplayListResult};
use display_list_builder::{NoDisplayListBuildingResult, StackingContextResult};
use floats::Floats;
use flow_list::{FlowList, FlowListIterator, MutFlowListIterator};
use flow_ref::FlowRef;
use fragment::{Fragment, FragmentBoundsIterator, TableRowFragment, TableCellFragment};
use incremental::{RECONSTRUCT_FLOW, REFLOW, REFLOW_OUT_OF_FLOW, RestyleDamage};
use inline::InlineFlow;
use model::{CollapsibleMargins, IntrinsicISizes, MarginCollapseInfo};
use parallel::FlowParallelInfo;
use table::{ColumnComputedInlineSize, ColumnIntrinsicInlineSize, TableFlow};
use table_caption::TableCaptionFlow;
use table_cell::TableCellFlow;
use table_colgroup::TableColGroupFlow;
use table_row::TableRowFlow;
use table_rowgroup::TableRowGroupFlow;
use table_wrapper::TableWrapperFlow;
use wrapper::ThreadSafeLayoutNode;

use geom::{Point2D, Rect, Size2D};
use serialize::{Encoder, Encodable};
use servo_msg::compositor_msg::LayerId;
use servo_util::geometry::Au;
use servo_util::logical_geometry::WritingMode;
use servo_util::logical_geometry::{LogicalRect, LogicalSize};
use std::mem;
use std::num::Zero;
use std::fmt;
use std::iter::Zip;
use std::raw;
use std::sync::atomic::{AtomicUint, SeqCst};
use std::slice::MutItems;
use style::computed_values::{clear, float, position, text_align};
use style::ComputedValues;
use sync::Arc;

/// Virtual methods that make up a float context.
///
/// Note that virtual methods have a cost; we should not overuse them in Servo. Consider adding
/// methods to `ImmutableFlowUtils` or `MutableFlowUtils` before adding more methods here.
pub trait Flow: fmt::Show + ToString + Sync {
    // RTTI
    //
    // TODO(pcwalton): Use Rust's RTTI, once that works.

    /// Returns the class of flow that this is.
    fn class(&self) -> FlowClass;

    /// If this is a block flow, returns the underlying object, borrowed immutably. Fails
    /// otherwise.
    fn as_immutable_block<'a>(&'a self) -> &'a BlockFlow {
        panic!("called as_immutable_block() on a non-block flow")
    }

    /// If this is a block flow, returns the underlying object. Fails otherwise.
    fn as_block<'a>(&'a mut self) -> &'a mut BlockFlow {
        debug!("called as_block() on a flow of type {}", self.class());
        panic!("called as_block() on a non-block flow")
    }

    /// If this is an inline flow, returns the underlying object, borrowed immutably. Fails
    /// otherwise.
    fn as_immutable_inline<'a>(&'a self) -> &'a InlineFlow {
        panic!("called as_immutable_inline() on a non-inline flow")
    }

    /// If this is an inline flow, returns the underlying object. Fails otherwise.
    fn as_inline<'a>(&'a mut self) -> &'a mut InlineFlow {
        panic!("called as_inline() on a non-inline flow")
    }

    /// If this is a table wrapper flow, returns the underlying object. Fails otherwise.
    fn as_table_wrapper<'a>(&'a mut self) -> &'a mut TableWrapperFlow {
        panic!("called as_table_wrapper() on a non-tablewrapper flow")
    }

    /// If this is a table wrapper flow, returns the underlying object, borrowed immutably. Fails
    /// otherwise.
    fn as_immutable_table_wrapper<'a>(&'a self) -> &'a TableWrapperFlow {
        panic!("called as_immutable_table_wrapper() on a non-tablewrapper flow")
    }

    /// If this is a table flow, returns the underlying object. Fails otherwise.
    fn as_table<'a>(&'a mut self) -> &'a mut TableFlow {
        panic!("called as_table() on a non-table flow")
    }

    /// If this is a table flow, returns the underlying object, borrowed immutably. Fails otherwise.
    fn as_immutable_table<'a>(&'a self) -> &'a TableFlow {
        panic!("called as_table() on a non-table flow")
    }

    /// If this is a table colgroup flow, returns the underlying object. Fails otherwise.
    fn as_table_colgroup<'a>(&'a mut self) -> &'a mut TableColGroupFlow {
        panic!("called as_table_colgroup() on a non-tablecolgroup flow")
    }

    /// If this is a table rowgroup flow, returns the underlying object. Fails otherwise.
    fn as_table_rowgroup<'a>(&'a mut self) -> &'a mut TableRowGroupFlow {
        panic!("called as_table_rowgroup() on a non-tablerowgroup flow")
    }

    /// If this is a table rowgroup flow, returns the underlying object, borrowed immutably. Fails
    /// otherwise.
    fn as_immutable_table_rowgroup<'a>(&'a self) -> &'a TableRowGroupFlow {
        panic!("called as_table_rowgroup() on a non-tablerowgroup flow")
    }

    /// If this is a table row flow, returns the underlying object. Fails otherwise.
    fn as_table_row<'a>(&'a mut self) -> &'a mut TableRowFlow {
        panic!("called as_table_row() on a non-tablerow flow")
    }

    /// If this is a table row flow, returns the underlying object, borrowed immutably. Fails
    /// otherwise.
    fn as_immutable_table_row<'a>(&'a self) -> &'a TableRowFlow {
        panic!("called as_table_row() on a non-tablerow flow")
    }

    /// If this is a table cell flow, returns the underlying object. Fails otherwise.
    fn as_table_caption<'a>(&'a mut self) -> &'a mut TableCaptionFlow {
        panic!("called as_table_caption() on a non-tablecaption flow")
    }

    /// If this is a table cell flow, returns the underlying object. Fails otherwise.
    fn as_table_cell<'a>(&'a mut self) -> &'a mut TableCellFlow {
        panic!("called as_table_cell() on a non-tablecell flow")
    }

    /// If this is a table cell flow, returns the underlying object, borrowed immutably. Fails
    /// otherwise.
    fn as_immutable_table_cell<'a>(&'a self) -> &'a TableCellFlow {
        panic!("called as_table_cell() on a non-tablecell flow")
    }

    /// If this is a table row, table rowgroup, or table flow, returns column intrinsic
    /// inline-sizes. Fails otherwise.
    fn column_intrinsic_inline_sizes<'a>(&'a mut self) -> &'a mut Vec<ColumnIntrinsicInlineSize> {
        panic!("called column_intrinsic_inline_sizes() on non-table flow")
    }

    /// If this is a table row, table rowgroup, or table flow, returns column computed
    /// inline-sizes. Fails otherwise.
    fn column_computed_inline_sizes<'a>(&'a mut self) -> &'a mut Vec<ColumnComputedInlineSize> {
        panic!("called column_intrinsic_inline_sizes() on non-table flow")
    }

    // Main methods

    /// Pass 1 of reflow: computes minimum and preferred inline-sizes.
    ///
    /// Recursively (bottom-up) determine the flow's minimum and preferred inline-sizes. When
    /// called on this flow, all child flows have had their minimum and preferred inline-sizes set.
    /// This function must decide minimum/preferred inline-sizes based on its children's inline-
    /// sizes and the dimensions of any boxes it is responsible for flowing.
    fn bubble_inline_sizes(&mut self) {
        panic!("bubble_inline_sizes not yet implemented")
    }

    /// Pass 2 of reflow: computes inline-size.
    fn assign_inline_sizes(&mut self, _ctx: &LayoutContext) {
        panic!("assign_inline_sizes not yet implemented")
    }

    /// Pass 3a of reflow: computes block-size.
    fn assign_block_size<'a>(&mut self, _ctx: &'a LayoutContext<'a>) {
        panic!("assign_block_size not yet implemented")
    }

    /// If this is a float, places it. The default implementation does nothing.
    fn place_float_if_applicable<'a>(&mut self, _: &'a LayoutContext<'a>) {}

    /// Assigns block-sizes in-order; or, if this is a float, places the float. The default
    /// implementation simply assigns block-sizes if this flow is impacted by floats. Returns true
    /// if this child was impacted by floats or false otherwise.
    fn assign_block_size_for_inorder_child_if_necessary<'a>(&mut self,
                                                            layout_context: &'a LayoutContext<'a>)
                                                            -> bool {
        let impacted = base(&*self).flags.impacted_by_floats();
        if impacted {
            self.assign_block_size(layout_context);
            mut_base(&mut *self).restyle_damage.remove(REFLOW_OUT_OF_FLOW | REFLOW);
        }
        impacted
    }

    /// Phase 4 of reflow: computes absolute positions.
    fn compute_absolute_position(&mut self) {
        // The default implementation is a no-op.
    }

    /// Phase 5 of reflow: builds display lists.
    fn build_display_list(&mut self, layout_context: &LayoutContext);

    /// Perform an iteration of fragment bounds on this flow.
    fn iterate_through_fragment_bounds(&self, iterator: &mut FragmentBoundsIterator);

    fn compute_collapsible_block_start_margin(&mut self,
                                              _layout_context: &mut LayoutContext,
                                              _margin_collapse_info: &mut MarginCollapseInfo) {
        // The default implementation is a no-op.
    }

    /// Marks this flow as the root flow. The default implementation is a no-op.
    fn mark_as_root(&mut self) {}

    // Note that the following functions are mostly called using static method
    // dispatch, so it's ok to have them in this trait. Plus, they have
    // different behaviour for different types of Flow, so they can't go into
    // the Immutable / Mutable Flow Utils traits without additional casts.

    /// Return true if store overflow is delayed for this flow.
    ///
    /// Currently happens only for absolutely positioned flows.
    fn is_store_overflow_delayed(&mut self) -> bool {
        false
    }

    fn is_root(&self) -> bool {
        false
    }

    /// The 'position' property of this flow.
    fn positioning(&self) -> position::T {
        position::static_
    }

    /// Return true if this flow has position 'fixed'.
    fn is_fixed(&self) -> bool {
        self.positioning() == position::fixed
    }

    fn is_positioned(&self) -> bool {
        self.is_relatively_positioned() || base(self).flags.contains(IS_ABSOLUTELY_POSITIONED)
    }

    fn is_relatively_positioned(&self) -> bool {
        self.positioning() == position::relative
    }

    /// Return true if this is the root of an absolute flow tree.
    fn is_root_of_absolute_flow_tree(&self) -> bool {
        false
    }

    /// Returns true if this is an absolute containing block.
    fn is_absolute_containing_block(&self) -> bool {
        false
    }

    /// Updates the inline position of a child flow during the assign-height traversal. At present,
    /// this is only used for absolutely-positioned inline-blocks.
    fn update_late_computed_inline_position_if_necessary(&mut self, inline_position: Au);

    /// Updates the block position of a child flow during the assign-height traversal. At present,
    /// this is only used for absolutely-positioned inline-blocks.
    fn update_late_computed_block_position_if_necessary(&mut self, block_position: Au);

    /// Return the dimensions of the containing block generated by this flow for absolutely-
    /// positioned descendants. For block flows, this is the padding box.
    ///
    /// NB: Do not change this `&self` to `&mut self` under any circumstances! It has security
    /// implications because this can be called on parents concurrently from descendants!
    fn generated_containing_block_rect(&self) -> LogicalRect<Au> {
        panic!("generated_containing_block_position not yet implemented for this flow")
    }

    /// Returns a layer ID for the given fragment.
    fn layer_id(&self, fragment_id: uint) -> LayerId {
        unsafe {
            let pointer: uint = mem::transmute(self);
            LayerId(pointer, fragment_id)
        }
    }

    /// Attempts to perform incremental fixup of this flow by replacing its fragment's style with
    /// the new style. This can only succeed if the flow has exactly one fragment.
    fn repair_style(&mut self, new_style: &Arc<ComputedValues>);
}

// Base access

#[inline(always)]
pub fn base<'a>(this: &'a Flow) -> &'a BaseFlow {
    unsafe {
        let obj = mem::transmute::<&'a Flow, raw::TraitObject>(this);
        mem::transmute::<*mut (), &'a BaseFlow>(obj.data)
    }
}

/// Iterates over the children of this immutable flow.
pub fn imm_child_iter<'a>(flow: &'a Flow) -> FlowListIterator<'a> {
    base(flow).children.iter()
}

#[inline(always)]
pub fn mut_base<'a>(this: &'a mut Flow) -> &'a mut BaseFlow {
    unsafe {
        let obj = mem::transmute::<&'a mut Flow, raw::TraitObject>(this);
        mem::transmute::<*mut (), &'a mut BaseFlow>(obj.data)
    }
}

/// Iterates over the children of this flow.
pub fn child_iter<'a>(flow: &'a mut Flow) -> MutFlowListIterator<'a> {
    mut_base(flow).children.iter_mut()
}

pub trait ImmutableFlowUtils {
    // Convenience functions

    /// Returns true if this flow is a block or a float flow.
    fn is_block_like(self) -> bool;

    /// Returns true if this flow is a table flow.
    fn is_table(self) -> bool;

    /// Returns true if this flow is a table caption flow.
    fn is_table_caption(self) -> bool;

    /// Returns true if this flow is a proper table child.
    fn is_proper_table_child(self) -> bool;

    /// Returns true if this flow is a table row flow.
    fn is_table_row(self) -> bool;

    /// Returns true if this flow is a table cell flow.
    fn is_table_cell(self) -> bool;

    /// Returns true if this flow is a table colgroup flow.
    fn is_table_colgroup(self) -> bool;

    /// Returns true if this flow is a table rowgroup flow.
    fn is_table_rowgroup(self) -> bool;

    /// Returns true if this flow is one of table-related flows.
    fn is_table_kind(self) -> bool;

    /// Returns true if anonymous flow is needed between this flow and child flow.
    fn need_anonymous_flow(self, child: &Flow) -> bool;

    /// Generates missing child flow of this flow.
    fn generate_missing_child_flow(self, node: &ThreadSafeLayoutNode) -> FlowRef;

    /// Returns true if this flow has no children.
    fn is_leaf(self) -> bool;

    /// Returns the number of children that this flow possesses.
    fn child_count(self) -> uint;

    /// Return true if this flow is a Block Container.
    fn is_block_container(self) -> bool;

    /// Returns true if this flow is a block flow.
    fn is_block_flow(self) -> bool;

    /// Returns true if this flow is an inline flow.
    fn is_inline_flow(self) -> bool;

    /// Dumps the flow tree for debugging.
    fn dump(self);

    /// Dumps the flow tree for debugging, with a prefix to indicate that we're at the given level.
    fn dump_with_level(self, level: uint);
}

pub trait MutableFlowUtils {
    // Traversals

    /// Traverses the tree in preorder.
    fn traverse_preorder<T:PreorderFlowTraversal>(self, traversal: &T);

    /// Traverses the tree in postorder.
    fn traverse_postorder<T:PostorderFlowTraversal>(self, traversal: &T);

    // Mutators

    /// Computes the overflow region for this flow.
    fn store_overflow(self, _: &LayoutContext);

    /// Gathers static block-offsets bubbled up by kids.
    ///
    /// This essentially gives us offsets of all absolutely positioned direct descendants and all
    /// fixed descendants, in tree order.
    ///
    /// This is called in a bottom-up traversal (specifically, the assign-block-size traversal).
    /// So, kids have their flow origin already set. In the case of absolute flow kids, they have
    /// their hypothetical box position already set.
    fn collect_static_block_offsets_from_children(self);
}

pub trait MutableOwnedFlowUtils {
    /// Set absolute descendants for this flow.
    ///
    /// Set this flow as the Containing Block for all the absolute descendants.
    fn set_absolute_descendants(&mut self, abs_descendants: AbsDescendants);
}

#[deriving(Encodable, PartialEq, Show)]
pub enum FlowClass {
    BlockFlowClass,
    InlineFlowClass,
    ListItemFlowClass,
    TableWrapperFlowClass,
    TableFlowClass,
    TableColGroupFlowClass,
    TableRowGroupFlowClass,
    TableRowFlowClass,
    TableCaptionFlowClass,
    TableCellFlowClass,
}

/// A top-down traversal.
pub trait PreorderFlowTraversal {
    /// The operation to perform. Return true to continue or false to stop.
    fn process(&self, flow: &mut Flow);

    /// Returns true if this node must be processed in-order. If this returns false,
    /// we skip the operation for this node, but continue processing the descendants.
    /// This is called *after* parent nodes are visited.
    fn should_process(&self, _flow: &mut Flow) -> bool {
        true
    }
}

/// A bottom-up traversal, with a optional in-order pass.
pub trait PostorderFlowTraversal {
    /// The operation to perform. Return true to continue or false to stop.
    fn process(&self, flow: &mut Flow);

    /// Returns false if this node must be processed in-order. If this returns false, we skip the
    /// operation for this node, but continue processing the ancestors. This is called *after*
    /// child nodes are visited.
    fn should_process(&self, _flow: &mut Flow) -> bool {
        true
    }
}

bitflags! {
    #[doc = "Flags used in flows."]
    flags FlowFlags: u16 {
        // floated descendants flags
        #[doc = "Whether this flow has descendants that float left in the same block formatting"]
        #[doc = "context."]
        const HAS_LEFT_FLOATED_DESCENDANTS = 0b0000_0000_0000_0001,
        #[doc = "Whether this flow has descendants that float right in the same block formatting"]
        #[doc = "context."]
        const HAS_RIGHT_FLOATED_DESCENDANTS = 0b0000_0000_0000_0010,
        #[doc = "Whether this flow is impacted by floats to the left in the same block formatting"]
        #[doc = "context (i.e. its height depends on some prior flows with `float: left`)."]
        const IMPACTED_BY_LEFT_FLOATS = 0b0000_0000_0000_0100,
        #[doc = "Whether this flow is impacted by floats to the right in the same block"]
        #[doc = "formatting context (i.e. its height depends on some prior flows with `float:"]
        #[doc = "right`)."]
        const IMPACTED_BY_RIGHT_FLOATS = 0b0000_0000_0000_1000,

        // text align flags
        #[doc = "Whether this flow contains a flow that has its own layer within the same absolute"]
        #[doc = "containing block."]
        const LAYERS_NEEDED_FOR_DESCENDANTS = 0b0000_0000_0001_0000,
        #[doc = "Whether this flow must have its own layer. Even if this flag is not set, it might"]
        #[doc = "get its own layer if it's deemed to be likely to overlap flows with their own"]
        #[doc = "layer."]
        const NEEDS_LAYER = 0b0000_0000_0010_0000,
        #[doc = "Whether this flow is absolutely positioned. This is checked all over layout, so a"]
        #[doc = "virtual call is too expensive."]
        const IS_ABSOLUTELY_POSITIONED = 0b0000_0000_0100_0000,
        #[doc = "Whether this flow clears to the left. This is checked all over layout, so a"]
        #[doc = "virtual call is too expensive."]
        const CLEARS_LEFT = 0b0000_0000_1000_0000,
        #[doc = "Whether this flow clears to the right. This is checked all over layout, so a"]
        #[doc = "virtual call is too expensive."]
        const CLEARS_RIGHT = 0b0000_0001_0000_0000,
        #[doc = "Whether this flow is left-floated. This is checked all over layout, so a"]
        #[doc = "virtual call is too expensive."]
        const FLOATS_LEFT = 0b0000_0010_0000_0000,
        #[doc = "Whether this flow is right-floated. This is checked all over layout, so a"]
        #[doc = "virtual call is too expensive."]
        const FLOATS_RIGHT = 0b0000_0100_0000_0000,
        #[doc = "Text alignment. \

                 NB: If you update this, update `TEXT_ALIGN_SHIFT` below."]
        const TEXT_ALIGN = 0b0111_1000_0000_0000,
    }
}

// NB: If you update this field, you must update the the floated descendants flags.
/// The bitmask of flags that represent the `has_left_floated_descendants` and
/// `has_right_floated_descendants` fields.

static HAS_FLOATED_DESCENDANTS_BITMASK: FlowFlags = FlowFlags { bits: 0b0000_0011 };

/// The number of bits we must shift off to handle the text alignment field.
///
/// NB: If you update this, update `TEXT_ALIGN` above.
static TEXT_ALIGN_SHIFT: uint = 11;

impl FlowFlags {
    /// Propagates text alignment flags from an appropriate parent flow per CSS 2.1.
    ///
    /// FIXME(#2265, pcwalton): It would be cleaner and faster to make this a derived CSS property
    /// `-servo-text-align-in-effect`.
    pub fn propagate_text_alignment_from_parent(&mut self, parent_flags: FlowFlags) {
        self.set_text_align_override(parent_flags);
    }

    #[inline]
    pub fn text_align(self) -> text_align::T {
        FromPrimitive::from_u16((self & TEXT_ALIGN).bits() >> TEXT_ALIGN_SHIFT).unwrap()
    }

    #[inline]
    pub fn set_text_align(&mut self, value: text_align::T) {
        *self = (*self & !TEXT_ALIGN) |
            FlowFlags::from_bits(value as u16 << TEXT_ALIGN_SHIFT).unwrap();
    }

    #[inline]
    pub fn set_text_align_override(&mut self, parent: FlowFlags) {
        self.insert(parent & TEXT_ALIGN);
    }

    #[inline]
    pub fn union_floated_descendants_flags(&mut self, other: FlowFlags) {
        self.insert(other & HAS_FLOATED_DESCENDANTS_BITMASK);
    }

    #[inline]
    pub fn impacted_by_floats(&self) -> bool {
        self.contains(IMPACTED_BY_LEFT_FLOATS) || self.contains(IMPACTED_BY_RIGHT_FLOATS)
    }

    #[inline]
    pub fn set(&mut self, flags: FlowFlags, value: bool) {
        if value {
            self.insert(flags);
        } else {
            self.remove(flags);
        }
    }

    #[inline]
    pub fn float_kind(&self) -> float::T {
        if self.contains(FLOATS_LEFT) {
            float::left
        } else if self.contains(FLOATS_RIGHT) {
            float::right
        } else {
            float::none
        }
    }

    #[inline]
    pub fn is_float(&self) -> bool {
        self.contains(FLOATS_LEFT) || self.contains(FLOATS_RIGHT)
    }

    #[inline]
    pub fn clears_floats(&self) -> bool {
        self.contains(CLEARS_LEFT) || self.contains(CLEARS_RIGHT)
    }
}

/// The Descendants of a flow.
///
/// Also, details about their position wrt this flow.
#[deriving(Clone)]
pub struct Descendants {
    /// Links to every descendant. This must be private because it is unsafe to leak `FlowRef`s to
    /// layout.
    descendant_links: Vec<FlowRef>,

    /// Static block-direction offsets of all descendants from the start of this flow box.
    pub static_block_offsets: Vec<Au>,
}

impl Descendants {
    pub fn new() -> Descendants {
        Descendants {
            descendant_links: Vec::new(),
            static_block_offsets: Vec::new(),
        }
    }

    pub fn len(&self) -> uint {
        self.descendant_links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descendant_links.is_empty()
    }

    pub fn push(&mut self, given_descendant: FlowRef) {
        self.descendant_links.push(given_descendant);
    }

    /// Push the given descendants on to the existing descendants.
    ///
    /// Ignore any static y offsets, because they are None before layout.
    pub fn push_descendants(&mut self, given_descendants: Descendants) {
        for elem in given_descendants.descendant_links.into_iter() {
            self.descendant_links.push(elem);
        }
    }

    /// Return an iterator over the descendant flows.
    pub fn iter<'a>(&'a mut self) -> DescendantIter<'a> {
        DescendantIter {
            iter: self.descendant_links.slice_from_mut(0).iter_mut(),
        }
    }

    /// Return an iterator over (descendant, static y offset).
    pub fn iter_with_offset<'a>(&'a mut self) -> DescendantOffsetIter<'a> {
        let descendant_iter = DescendantIter {
            iter: self.descendant_links.slice_from_mut(0).iter_mut(),
        };
        descendant_iter.zip(self.static_block_offsets.slice_from_mut(0).iter_mut())
    }
}

pub type AbsDescendants = Descendants;

pub struct DescendantIter<'a> {
    iter: MutItems<'a, FlowRef>,
}

impl<'a> Iterator<&'a mut Flow + 'a> for DescendantIter<'a> {
    fn next(&mut self) -> Option<&'a mut Flow + 'a> {
        self.iter.next().map(|flow| &mut **flow)
    }
}

pub type DescendantOffsetIter<'a> = Zip<DescendantIter<'a>, MutItems<'a, Au>>;

/// Information needed to compute absolute (i.e. viewport-relative) flow positions (not to be
/// confused with absolutely-positioned flows).
#[deriving(Encodable)]
pub struct AbsolutePositionInfo {
    /// The size of the containing block for relatively-positioned descendants.
    pub relative_containing_block_size: LogicalSize<Au>,

    /// The position of the absolute containing block relative to the nearest ancestor stacking
    /// context. If the absolute containing block establishes the stacking context for this flow,
    /// and this flow is not itself absolutely-positioned, then this is (0, 0).
    pub stacking_relative_position_of_absolute_containing_block: Point2D<Au>,

    /// Whether the absolute containing block forces positioned descendants to be layerized.
    ///
    /// FIXME(pcwalton): Move into `FlowFlags`.
    pub layers_needed_for_positioned_flows: bool,
}

impl AbsolutePositionInfo {
    pub fn new(writing_mode: WritingMode) -> AbsolutePositionInfo {
        // FIXME(pcwalton): The initial relative containing block-size should be equal to the size
        // of the root layer.
        AbsolutePositionInfo {
            relative_containing_block_size: LogicalSize::zero(writing_mode),
            stacking_relative_position_of_absolute_containing_block: Zero::zero(),
            layers_needed_for_positioned_flows: false,
        }
    }
}

/// Data common to all flows.
pub struct BaseFlow {
    /// NB: Must be the first element.
    ///
    /// The necessity of this will disappear once we have dynamically-sized types.
    ref_count: AtomicUint,

    pub restyle_damage: RestyleDamage,

    /// The children of this flow.
    pub children: FlowList,

    /// Intrinsic inline sizes for this flow.
    pub intrinsic_inline_sizes: IntrinsicISizes,

    /// The upper left corner of the box representing this flow, relative to the box representing
    /// its parent flow.
    ///
    /// For absolute flows, this represents the position with respect to its *containing block*.
    ///
    /// This does not include margins in the block flow direction, because those can collapse. So
    /// for the block direction (usually vertical), this represents the *border box*. For the
    /// inline direction (usually horizontal), this represents the *margin box*.
    pub position: LogicalRect<Au>,

    /// The amount of overflow of this flow, relative to the containing block. Must include all the
    /// pixels of all the display list items for correct invalidation.
    pub overflow: LogicalRect<Au>,

    /// Data used during parallel traversals.
    ///
    /// TODO(pcwalton): Group with other transient data to save space.
    pub parallel: FlowParallelInfo,

    /// The floats next to this flow.
    pub floats: Floats,

    /// The collapsible margins for this flow, if any.
    pub collapsible_margins: CollapsibleMargins,

    /// The position of this flow relative to the start of the nearest ancestor stacking context.
    /// This is computed during the top-down pass of display list construction.
    pub stacking_relative_position: Point2D<Au>,

    /// Details about descendants with position 'absolute' or 'fixed' for which we are the
    /// containing block. This is in tree order. This includes any direct children.
    pub abs_descendants: AbsDescendants,

    /// The inline-size of the block container of this flow. Used for computing percentage and
    /// automatic values for `width`.
    pub block_container_inline_size: Au,

    /// The block-size of the block container of this flow, if it is an explicit size (does not
    /// depend on content heights).  Used for computing percentage values for `height`.
    pub block_container_explicit_block_size: Option<Au>,

    /// Offset wrt the nearest positioned ancestor - aka the Containing Block
    /// for any absolutely positioned elements.
    pub absolute_static_i_offset: Au,

    /// Offset wrt the Initial Containing Block.
    pub fixed_static_i_offset: Au,

    /// Reference to the Containing Block, if this flow is absolutely positioned.
    pub absolute_cb: ContainingBlockLink,

    /// Information needed to compute absolute (i.e. viewport-relative) flow positions (not to be
    /// confused with absolutely-positioned flows).
    ///
    /// FIXME(pcwalton): Merge with `absolute_static_i_offset` and `fixed_static_i_offset` above?
    pub absolute_position_info: AbsolutePositionInfo,

    /// The clipping rectangle for this flow and its descendants, in layer coordinates.
    ///
    /// TODO(pcwalton): When we have `border-radius` this will need to at least support rounded
    /// rectangles.
    pub clip_rect: Rect<Au>,

    /// The results of display list building for this flow.
    pub display_list_building_result: DisplayListBuildingResult,

    /// The writing mode for this flow.
    pub writing_mode: WritingMode,

    /// Various flags for flows, tightly packed to save space.
    pub flags: FlowFlags,
}

impl fmt::Show for BaseFlow {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "@ {}, CC {}, ADC {}",
               self.position,
               self.parallel.children_count.load(SeqCst),
               self.abs_descendants.len())
    }
}

impl<E, S: Encoder<E>> Encodable<S, E> for BaseFlow {
    fn encode(&self, e: &mut S) -> Result<(), E> {
        e.emit_struct("base", 0, |e| {
            try!(e.emit_struct_field("id", 0, |e| self.debug_id().encode(e)))
            try!(e.emit_struct_field("stacking_relative_position",
                                     1,
                                     |e| self.stacking_relative_position.encode(e)))
            try!(e.emit_struct_field("intrinsic_inline_sizes",
                                     2,
                                     |e| self.intrinsic_inline_sizes.encode(e)))
            try!(e.emit_struct_field("position", 3, |e| self.position.encode(e)))
            e.emit_struct_field("children", 4, |e| {
                e.emit_seq(self.children.len(), |e| {
                    for (i, c) in self.children.iter().enumerate() {
                        try!(e.emit_seq_elt(i, |e| {
                            try!(e.emit_struct("flow", 0, |e| {
                                try!(e.emit_struct_field("class", 0, |e| c.class().encode(e)))
                                e.emit_struct_field("data", 1, |e| {
                                    match c.class() {
                                        BlockFlowClass => c.as_immutable_block().encode(e),
                                        InlineFlowClass => c.as_immutable_inline().encode(e),
                                        TableFlowClass => c.as_immutable_table().encode(e),
                                        TableWrapperFlowClass => c.as_immutable_table_wrapper().encode(e),
                                        TableRowGroupFlowClass => c.as_immutable_table_rowgroup().encode(e),
                                        TableRowFlowClass => c.as_immutable_table_row().encode(e),
                                        TableCellFlowClass => c.as_immutable_table_cell().encode(e),
                                        _ => { Ok(()) }     // TODO: Support captions
                                    }
                                })
                            }))
                            Ok(())
                        }))
                    }
                    Ok(())
                })

            })
        })
    }
}

#[unsafe_destructor]
impl Drop for BaseFlow {
    fn drop(&mut self) {
        if self.ref_count.load(SeqCst) != 0 {
            panic!("Flow destroyed before its ref count hit zero—this is unsafe!")
        }
    }
}

/// Whether a base flow should be forced to be nonfloated. This can affect e.g. `TableFlow`, which
/// is never floated because the table wrapper flow is the floated one.
#[deriving(Clone, PartialEq)]
pub enum ForceNonfloatedFlag {
    /// The flow should be floated if the node has a `float` property.
    FloatIfNecessary,
    /// The flow should be forced to be nonfloated.
    ForceNonfloated,
}

impl BaseFlow {
    #[inline]
    pub fn new(node: Option<ThreadSafeLayoutNode>,
               writing_mode: WritingMode,
               force_nonfloated: ForceNonfloatedFlag)
               -> BaseFlow {
        let mut flags = FlowFlags::empty();
        match node {
            None => {}
            Some(node) => {
                let node_style = node.style();
                match node_style.get_box().position {
                    position::absolute | position::fixed => {
                        flags.insert(IS_ABSOLUTELY_POSITIONED)
                    }
                    _ => {}
                }

                if force_nonfloated == FloatIfNecessary {
                    match node_style.get_box().float {
                        float::none => {}
                        float::left => flags.insert(FLOATS_LEFT),
                        float::right => flags.insert(FLOATS_RIGHT),
                    }
                }

                match node_style.get_box().clear {
                    clear::none => {}
                    clear::left => flags.insert(CLEARS_LEFT),
                    clear::right => flags.insert(CLEARS_RIGHT),
                    clear::both => {
                        flags.insert(CLEARS_LEFT);
                        flags.insert(CLEARS_RIGHT);
                    }
                }
            }
        }

        // New flows start out as fully damaged.
        let mut damage = RestyleDamage::all();
        damage.remove(RECONSTRUCT_FLOW);

        BaseFlow {
            ref_count: AtomicUint::new(1),
            restyle_damage: damage,
            children: FlowList::new(),
            intrinsic_inline_sizes: IntrinsicISizes::new(),
            position: LogicalRect::zero(writing_mode),
            overflow: LogicalRect::zero(writing_mode),
            parallel: FlowParallelInfo::new(),
            floats: Floats::new(writing_mode),
            collapsible_margins: CollapsibleMargins::new(),
            stacking_relative_position: Zero::zero(),
            abs_descendants: Descendants::new(),
            absolute_static_i_offset: Au(0),
            fixed_static_i_offset: Au(0),
            block_container_inline_size: Au(0),
            block_container_explicit_block_size: None,
            absolute_cb: ContainingBlockLink::new(),
            display_list_building_result: NoDisplayListBuildingResult,
            absolute_position_info: AbsolutePositionInfo::new(writing_mode),
            clip_rect: Rect(Zero::zero(), Size2D(Au(0), Au(0))),
            flags: flags,
            writing_mode: writing_mode,
        }
    }

    pub fn child_iter<'a>(&'a mut self) -> MutFlowListIterator<'a> {
        self.children.iter_mut()
    }

    pub unsafe fn ref_count<'a>(&'a self) -> &'a AtomicUint {
        &self.ref_count
    }

    pub fn debug_id(&self) -> uint {
        let p = self as *const _;
        p as uint
    }

    /// Ensures that all display list items generated by this flow are within the flow's overflow
    /// rect. This should only be used for debugging.
    pub fn validate_display_list_geometry(&self) {
        let position_with_overflow = self.position.union(&self.overflow);
        let bounds = Rect(self.stacking_relative_position,
                          Size2D(position_with_overflow.size.inline,
                                 position_with_overflow.size.block));

        let all_items = match self.display_list_building_result {
            NoDisplayListBuildingResult => Vec::new(),
            StackingContextResult(ref stacking_context) => {
                stacking_context.display_list.all_display_items()
            }
            DisplayListResult(ref display_list) => display_list.all_display_items(),
        };

        for item in all_items.iter() {
            let paint_bounds = match item.base().bounds.intersection(&item.base().clip_rect) {
                None => continue,
                Some(rect) => rect,
            };

            if paint_bounds.is_empty() {
                continue;
            }

            if bounds.union(&paint_bounds) != bounds {
                error!("DisplayList item {} outside of Flow overflow ({})", item, paint_bounds);
            }
        }
    }

    /// Returns the position of the given fragment relative to the start of the nearest ancestor
    /// stacking context. The fragment must be a child fragment of this flow.
    pub fn stacking_relative_position_of_child_fragment(&self, fragment: &Fragment)
                                                        -> Point2D<Au> {
        let relative_offset =
            fragment.relative_position(&self
                                       .absolute_position_info
                                       .relative_containing_block_size);
        self.stacking_relative_position.add_size(&relative_offset.to_physical(self.writing_mode))
    }
}

impl<'a> ImmutableFlowUtils for &'a Flow + 'a {
    /// Returns true if this flow is a block flow.
    fn is_block_like(self) -> bool {
        match self.class() {
            BlockFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a proper table child.
    /// 'Proper table child' is defined as table-row flow, table-rowgroup flow,
    /// table-column-group flow, or table-caption flow.
    fn is_proper_table_child(self) -> bool {
        match self.class() {
            TableRowFlowClass | TableRowGroupFlowClass |
                TableColGroupFlowClass | TableCaptionFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a table row flow.
    fn is_table_row(self) -> bool {
        match self.class() {
            TableRowFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a table cell flow.
    fn is_table_cell(self) -> bool {
        match self.class() {
            TableCellFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a table colgroup flow.
    fn is_table_colgroup(self) -> bool {
        match self.class() {
            TableColGroupFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a table flow.
    fn is_table(self) -> bool {
        match self.class() {
            TableFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a table caption flow.
    fn is_table_caption(self) -> bool {
        match self.class() {
            TableCaptionFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is a table rowgroup flow.
    fn is_table_rowgroup(self) -> bool {
        match self.class() {
            TableRowGroupFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is one of table-related flows.
    fn is_table_kind(self) -> bool {
        match self.class() {
            TableWrapperFlowClass | TableFlowClass |
                TableColGroupFlowClass | TableRowGroupFlowClass |
                TableRowFlowClass | TableCaptionFlowClass | TableCellFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if anonymous flow is needed between this flow and child flow.
    /// Spec: http://www.w3.org/TR/CSS21/tables.html#anonymous-boxes
    fn need_anonymous_flow(self, child: &Flow) -> bool {
        match self.class() {
            TableFlowClass => !child.is_proper_table_child(),
            TableRowGroupFlowClass => !child.is_table_row(),
            TableRowFlowClass => !child.is_table_cell(),
            _ => false
        }
    }

    /// Generates missing child flow of this flow.
    fn generate_missing_child_flow(self, node: &ThreadSafeLayoutNode) -> FlowRef {
        let flow = match self.class() {
            TableFlowClass | TableRowGroupFlowClass => {
                let fragment = Fragment::new_anonymous_table_fragment(node, TableRowFragment);
                box TableRowFlow::from_node_and_fragment(node, fragment) as Box<Flow>
            },
            TableRowFlowClass => {
                let fragment = Fragment::new_anonymous_table_fragment(node, TableCellFragment);
                box TableCellFlow::from_node_and_fragment(node, fragment) as Box<Flow>
            },
            _ => {
                panic!("no need to generate a missing child")
            }
        };
        FlowRef::new(flow)
    }

    /// Returns true if this flow has no children.
    fn is_leaf(self) -> bool {
        base(self).children.len() == 0
    }

    /// Returns the number of children that this flow possesses.
    fn child_count(self) -> uint {
        base(self).children.len()
    }

    /// Return true if this flow is a Block Container.
    ///
    /// Except for table fragments and replaced elements, block-level fragments (`BlockFlow`) are
    /// also block container fragments.
    /// Non-replaced inline blocks and non-replaced table cells are also block
    /// containers.
    fn is_block_container(self) -> bool {
        match self.class() {
            // TODO: Change this when inline-blocks are supported.
            BlockFlowClass | TableCaptionFlowClass | TableCellFlowClass => {
                // FIXME: Actually check the type of the node
                self.child_count() != 0
            }
            _ => false,
        }
    }

    /// Returns true if this flow is a block flow.
    fn is_block_flow(self) -> bool {
        match self.class() {
            BlockFlowClass => true,
            _ => false,
        }
    }

    /// Returns true if this flow is an inline flow.
    fn is_inline_flow(self) -> bool {
        match self.class() {
            InlineFlowClass => true,
            _ => false,
        }
    }

    /// Dumps the flow tree for debugging.
    fn dump(self) {
        self.dump_with_level(0)
    }

    /// Dumps the flow tree for debugging, with a prefix to indicate that we're at the given level.
    fn dump_with_level(self, level: uint) {
        let mut indent = String::new();
        for _ in range(0, level) {
            indent.push_str("| ")
        }

        println!("{}+ {}", indent, self.to_string());

        for kid in imm_child_iter(self) {
            kid.dump_with_level(level + 1)
        }
    }
}

impl<'a> MutableFlowUtils for &'a mut Flow + 'a {
    /// Traverses the tree in preorder.
    fn traverse_preorder<T:PreorderFlowTraversal>(self, traversal: &T) {
        if traversal.should_process(self) {
            traversal.process(self);
        }

        for kid in child_iter(self) {
            kid.traverse_preorder(traversal);
        }
    }

    /// Traverses the tree in postorder.
    fn traverse_postorder<T:PostorderFlowTraversal>(self, traversal: &T) {
        for kid in child_iter(self) {
            kid.traverse_postorder(traversal);
        }

        if traversal.should_process(self) {
            traversal.process(self)
        }
    }

    /// Calculate and set overflow for current flow.
    ///
    /// CSS Section 11.1
    /// This is the union of rectangles of the flows for which we define the
    /// Containing Block.
    ///
    /// Assumption: This is called in a bottom-up traversal, so kids' overflows have
    /// already been set.
    /// Assumption: Absolute descendants have had their overflow calculated.
    fn store_overflow(self, _: &LayoutContext) {
        let my_position = mut_base(self).position;

        // FIXME(pcwalton): We should calculate overflow on a per-fragment basis, because their
        // styles can affect overflow regions. Consider `box-shadow`, `outline`, etc.--anything
        // that can draw outside the border box. For now we assume overflow is the border box, but
        // that is wrong.
        let mut overflow = my_position;

        if self.is_block_container() {
            for kid in child_iter(self) {
                if kid.is_store_overflow_delayed() {
                    // Absolute flows will be handled by their CB. If we are
                    // their CB, they will show up in `abs_descendants`.
                    continue;
                }
                let mut kid_overflow = base(kid).overflow;
                kid_overflow = kid_overflow.translate(&my_position.start);
                overflow = overflow.union(&kid_overflow)
            }

            // FIXME(#2004, pcwalton): This is wrong for `position: fixed`.
            for descendant_link in mut_base(self).abs_descendants.iter() {
                let mut kid_overflow = base(descendant_link).overflow;
                kid_overflow = kid_overflow.translate(&my_position.start);
                overflow = overflow.union(&kid_overflow)
            }
        }
        /*for box_shadow in self.style().get_effects().box_shadow.iter() {
            let inflation = (box_shadow.spread_radius + box_shadow.blur_radius) *
                BOX_SHADOW_INFLATION_FACTOR;
            result = my_position.translate(&Point2D(box_shadow.offset_x, box_shadow.offset_y))
                                .inflate(inflation, inflation).union(&result);
        }*/
        overflow.start.i = overflow.start.i - Au(6000);
        overflow.start.b = overflow.start.b - Au(6000);
        overflow.size.inline = overflow.size.inline + Au(6000*2);
        overflow.size.block = overflow.size.block + Au(6000*2);
        mut_base(self).overflow = overflow;
    }

    /// Collect and update static y-offsets bubbled up by kids.
    ///
    /// This would essentially give us offsets of all absolutely positioned
    /// direct descendants and all fixed descendants, in tree order.
    ///
    /// Assume that this is called in a bottom-up traversal (specifically, the
    /// assign-block-size traversal). So, kids have their flow origin already set.
    /// In the case of absolute flow kids, they have their hypothetical box
    /// position already set.
    fn collect_static_block_offsets_from_children(self) {
        let mut absolute_descendant_block_offsets = Vec::new();
        for kid in mut_base(self).child_iter() {
            let mut gives_absolute_offsets = true;
            if kid.is_block_like() {
                let kid_block = kid.as_block();
                if kid_block.is_fixed() || kid_block.base.flags.contains(IS_ABSOLUTELY_POSITIONED) {
                    // It won't contribute any offsets for descendants because it would be the
                    // containing block for them.
                    gives_absolute_offsets = false;
                    // Give the offset for the current absolute flow alone.
                    absolute_descendant_block_offsets.push(
                        kid_block.get_hypothetical_block_start_edge());
                } else if kid_block.is_positioned() {
                    // It won't contribute any offsets because it would be the containing block
                    // for the descendants.
                    gives_absolute_offsets = false;
                }
            }

            if gives_absolute_offsets {
                let kid_base = mut_base(kid);
                // Avoid copying the offset vector.
                let offsets = mem::replace(&mut kid_base.abs_descendants.static_block_offsets,
                                           Vec::new());
                // Consume all the static block-offsets bubbled up by kids.
                for block_offset in offsets.into_iter() {
                    // The offsets are with respect to the kid flow's fragment. Translate them to
                    // that of the current flow.
                    absolute_descendant_block_offsets.push(
                        block_offset + kid_base.position.start.b);
                }
            }
        }
        mut_base(self).abs_descendants.static_block_offsets = absolute_descendant_block_offsets
    }
}

impl MutableOwnedFlowUtils for FlowRef {
    /// Set absolute descendants for this flow.
    ///
    /// Set yourself as the Containing Block for all the absolute descendants.
    ///
    /// This is called during flow construction, so nothing else can be accessing the descendant
    /// flows. This is enforced by the fact that we have a mutable `FlowRef`, which only flow
    /// construction is allowed to possess.
    fn set_absolute_descendants(&mut self, abs_descendants: AbsDescendants) {
        let this = self.clone();

        let block = self.as_block();
        block.base.abs_descendants = abs_descendants;

        for descendant_link in block.base.abs_descendants.iter() {
            let base = mut_base(descendant_link);
            base.absolute_cb.set(this.clone());
        }
    }
}

/// A link to a flow's containing block.
///
/// This cannot safely be a `Flow` pointer because this is a pointer *up* the tree, not *down* the
/// tree. A pointer up the tree is unsafe during layout because it can be used to access a node
/// with an immutable reference while that same node is being laid out, causing possible iterator
/// invalidation and use-after-free.
///
/// FIXME(pcwalton): I think this would be better with a borrow flag instead of `unsafe`.
pub struct ContainingBlockLink {
    /// The pointer up to the containing block.
    link: Option<FlowRef>,
}

impl ContainingBlockLink {
    fn new() -> ContainingBlockLink {
        ContainingBlockLink {
            link: None,
        }
    }

    fn set(&mut self, link: FlowRef) {
        self.link = Some(link)
    }

    pub unsafe fn get<'a>(&'a mut self) -> &'a mut Option<FlowRef> {
        &mut self.link
    }

    #[inline]
    pub fn generated_containing_block_rect(&mut self) -> LogicalRect<Au> {
        match self.link {
            None => panic!("haven't done it"),
            Some(ref mut link) => link.generated_containing_block_rect(),
        }
    }
}
