//! Represents a 'basic block' of [`Instruction`]s in a control flow graph.
//!
//! [`Block`]s contain zero or more _non-terminating_ instructions and at most one _terminating_
//! instruction or _terminator_.  Terminators are either branches or a return instruction and are
//! the last instruction in the block.
//!
//! Blocks also contain a single 'phi' instruction at its start.  In
//! [SSA](https://en.wikipedia.org/wiki/Static_single_assignment_form) form 'phi' instructions are
//! used to merge values from preceding blocks.
//!
//! Every [`Function`] has at least one block, the first of which is usually labeled `entry`.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    context::Context,
    error::IrError,
    function::Function,
    instruction::{FuelVmInstruction, Instruction, InstructionInserter, InstructionIterator},
    pretty::DebugWithContext,
    value::{Value, ValueDatum},
    BranchToWithArgs, Type,
};

/// A wrapper around an [ECS](https://github.com/fitzgen/generational-arena) handle into the
/// [`Context`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, DebugWithContext)]
pub struct Block(pub generational_arena::Index);

#[doc(hidden)]
pub struct BlockContent {
    /// Block label, useful for printing.
    pub label: Label,
    /// The function containing this block.
    pub function: Function,
    /// List of instructions in the block.
    pub instructions: Vec<Value>,
    /// Block arguments: Another form of SSA PHIs.
    pub args: Vec<Value>,
    /// CFG predecessors
    pub preds: FxHashSet<Block>,
}

#[derive(Debug, Clone, DebugWithContext)]
pub struct BlockArgument {
    /// The block of which this is an argument.
    pub block: Block,
    /// idx'th argument of the block.
    pub idx: usize,
    pub ty: Type,
}

impl BlockArgument {
    /// Get the actual parameter passed to this block argument from `from_block`
    pub fn get_val_coming_from(&self, context: &Context, from_block: &Block) -> Option<Value> {
        for pred in self.block.pred_iter(context) {
            for BranchToWithArgs { block, args } in pred.successors(context) {
                if block == *from_block {
                    return Some(args[self.idx]);
                }
            }
        }
        None
    }
}

/// Each block may be explicitly named.  A [`Label`] is a simple `String` synonym.
pub type Label = String;

impl Block {
    /// Return a new block handle.
    ///
    /// Creates a new Block belonging to `function` in the context and returns its handle.  `label`
    /// is optional and is used only when printing the IR.
    pub fn new(context: &mut Context, function: Function, label: Option<String>) -> Block {
        let label = function.get_unique_label(context, label);
        let content = BlockContent {
            label,
            function,
            instructions: vec![],
            args: vec![],
            preds: FxHashSet::default(),
        };
        Block(context.blocks.insert(content))
    }

    /// Get the parent function for this block.
    pub fn get_function(&self, context: &Context) -> Function {
        context.blocks[self.0].function
    }

    /// Create a new [`InstructionIterator`] to more easily append instructions to this block.
    pub fn ins<'a>(&self, context: &'a mut Context) -> InstructionInserter<'a> {
        InstructionInserter::new(context, *self)
    }

    /// Get the label of this block.  If it wasn't given one upon creation it will be a generated
    /// label.
    pub fn get_label(&self, context: &Context) -> String {
        context.blocks[self.0].label.clone()
    }

    /// Get the number of instructions in this block
    pub fn num_instructions(&self, context: &Context) -> usize {
        context.blocks[self.0].instructions.len()
    }

    /// Get the i'th block arg.
    pub fn get_arg(&self, context: &Context, index: usize) -> Option<Value> {
        context.blocks[self.0].args.get(index).cloned()
    }

    /// Get the number of predecessor blocks, i.e., blocks which branch to this one.
    pub fn num_predecessors(&self, context: &Context) -> usize {
        context.blocks[self.0].preds.len()
    }

    /// Add a new block argument of type `ty`. Returns its index.
    pub fn new_arg(&self, context: &mut Context, ty: Type) -> usize {
        let idx = context.blocks[self.0].args.len();
        let arg_val = Value::new_argument(
            context,
            BlockArgument {
                block: *self,
                idx,
                ty,
            },
        );
        context.blocks[self.0].args.push(arg_val);
        idx
    }

    /// Add a block argument, asserts that `arg` is suitable here.
    pub fn add_arg(&self, context: &mut Context, arg: Value) {
        match context.values[arg.0].value {
            ValueDatum::Argument(BlockArgument { block, idx, ty: _ })
                if block == *self && idx == context.blocks[self.0].args.len() =>
            {
                context.blocks[self.0].args.push(arg);
            }
            _ => panic!("Inconsistent block argument being added"),
        }
    }

    /// Get an iterator over this block's args.
    pub fn arg_iter<'a>(&'a self, context: &'a Context) -> impl Iterator<Item = &Value> {
        context.blocks[self.0].args.iter()
    }

    /// How many args does this block have?
    pub fn num_args(&self, context: &Context) -> usize {
        context.blocks[self.0].args.len()
    }

    /// Get an iterator over this block's predecessor blocks.
    pub fn pred_iter<'a>(&'a self, context: &'a Context) -> impl Iterator<Item = &Block> {
        context.blocks[self.0].preds.iter()
    }

    /// Add `from_block` to the set of predecessors of this block.
    pub fn add_pred(&self, context: &mut Context, from_block: &Block) {
        context.blocks[self.0].preds.insert(*from_block);
    }

    /// Remove `from_block` from the set of predecessors of this block.
    pub fn remove_pred(&self, context: &mut Context, from_block: &Block) {
        context.blocks[self.0].preds.remove(from_block);
    }

    /// Replace a `old_source` with `new_source` as a predecessor.
    pub fn replace_pred(&self, context: &mut Context, old_source: &Block, new_source: &Block) {
        self.remove_pred(context, old_source);
        self.add_pred(context, new_source);
    }

    /// Get a reference to the block terminator.
    ///
    /// Returns `None` if block is empty.
    pub fn get_terminator<'a>(&self, context: &'a Context) -> Option<&'a Instruction> {
        context.blocks[self.0].instructions.last().and_then(|val| {
            // It's guaranteed to be an instruction value.
            if let ValueDatum::Instruction(term_inst) = &context.values[val.0].value {
                Some(term_inst)
            } else {
                None
            }
        })
    }

    /// Get a mut reference to the block terminator.
    ///
    /// Returns `None` if block is empty.
    pub fn get_terminator_mut<'a>(&self, context: &'a mut Context) -> Option<&'a mut Instruction> {
        context.blocks[self.0].instructions.last().and_then(|val| {
            // It's guaranteed to be an instruction value.
            if let ValueDatum::Instruction(term_inst) = &mut context.values[val.0].value {
                Some(term_inst)
            } else {
                None
            }
        })
    }

    /// Get the CFG successors (and the parameters passed to them) of this block.
    pub(super) fn successors<'a>(&'a self, context: &'a Context) -> Vec<BranchToWithArgs> {
        match self.get_terminator(context) {
            Some(Instruction::ConditionalBranch {
                true_block,
                false_block,
                ..
            }) => vec![true_block.clone(), false_block.clone()],

            Some(Instruction::Branch(block)) => vec![block.clone()],

            _otherwise => Vec::new(),
        }
    }

    /// For a particular successor (if it indeed is one), get the arguments passed.
    pub fn get_succ_params(&self, context: &Context, succ: &Block) -> Vec<Value> {
        self.successors(context)
            .iter()
            .find(|branch| &branch.block == succ)
            .map_or(vec![], |branch| branch.args.clone())
    }

    /// For a particular successor (if it indeed is one), get a mut ref to parameters passed.
    pub fn get_succ_params_mut<'a>(
        &'a self,
        context: &'a mut Context,
        succ: &Block,
    ) -> Option<&'a mut Vec<Value>> {
        match self.get_terminator_mut(context) {
            Some(Instruction::ConditionalBranch {
                true_block,
                false_block,
                ..
            }) => {
                if true_block.block == *succ {
                    Some(&mut true_block.args)
                } else if false_block.block == *succ {
                    Some(&mut false_block.args)
                } else {
                    None
                }
            }
            Some(Instruction::Branch(block)) if block.block == *succ => Some(&mut block.args),
            _ => None,
        }
    }

    /// Replace successor `old_succ` with `new_succ`.
    /// Updates `preds` of both `old_succ` and `new_succ`.
    pub(super) fn replace_successor(
        &self,
        context: &mut Context,
        old_succ: Block,
        new_succ: Block,
        new_params: Vec<Value>,
    ) {
        let mut modified = false;
        if let Some(term) = self.get_terminator_mut(context) {
            match term {
                Instruction::ConditionalBranch {
                    true_block:
                        BranchToWithArgs {
                            block: true_block,
                            args: true_opds,
                        },
                    false_block:
                        BranchToWithArgs {
                            block: false_block,
                            args: false_opds,
                        },
                    cond_value: _,
                } => {
                    if old_succ == *true_block {
                        modified = true;
                        *true_block = new_succ;
                        *true_opds = new_params.clone();
                    }
                    if old_succ == *false_block {
                        modified = true;
                        *false_block = new_succ;
                        *false_opds = new_params
                    }
                }

                Instruction::Branch(BranchToWithArgs { block, args }) if *block == old_succ => {
                    *block = new_succ;
                    *args = new_params;
                    modified = true;
                }
                _ => (),
            }
        }
        if modified {
            old_succ.remove_pred(context, self);
            new_succ.add_pred(context, self);
        }
    }

    /// Return whether this block is already terminated.  Checks if the final instruction, if it
    /// exists, is a terminator.
    pub fn is_terminated(&self, context: &Context) -> bool {
        context.blocks[self.0]
            .instructions
            .last()
            .map_or(false, |val| val.is_terminator(context))
    }

    /// Return whether this block is already terminated specifically by a Ret instruction.
    pub fn is_terminated_by_ret_or_revert(&self, context: &Context) -> bool {
        self.get_terminator(context).map_or(false, |i| {
            matches!(
                i,
                Instruction::Ret(..) | Instruction::FuelVm(FuelVmInstruction::Revert(..))
            )
        })
    }

    /// Replace a value within this block.
    ///
    /// For every instruction within the block, any reference to `old_val` is replaced with
    /// `new_val`.
    pub fn replace_values(&self, context: &mut Context, replace_map: &FxHashMap<Value, Value>) {
        for ins_idx in 0..context.blocks[self.0].instructions.len() {
            let ins = context.blocks[self.0].instructions[ins_idx];
            ins.replace_instruction_values(context, replace_map);
        }
    }

    /// Remove an instruction from this block.
    ///
    /// **NOTE:** We must be very careful!  We mustn't remove the phi or the terminator.  Some
    /// extra checks should probably be performed here to avoid corruption! Ideally we use get a
    /// user/uses system implemented.  Using `Vec::remove()` is also O(n) which we may want to
    /// avoid someday.
    pub fn remove_instruction(&self, context: &mut Context, instr_val: Value) {
        let ins = &mut context.blocks[self.0].instructions;
        if let Some(pos) = ins.iter().position(|iv| *iv == instr_val) {
            ins.remove(pos);
        }
    }

    /// Replace an instruction in this block with another.  Will return a ValueNotFound on error.
    /// Any use of the old instruction value will also be replaced by the new value throughout the
    /// owning function.
    pub fn replace_instruction(
        &self,
        context: &mut Context,
        old_instr_val: Value,
        new_instr_val: Value,
    ) -> Result<(), IrError> {
        match context.blocks[self.0]
            .instructions
            .iter_mut()
            .find(|instr_val| *instr_val == &old_instr_val)
        {
            None => Err(IrError::ValueNotFound(
                "Attempting to replace instruction.".to_owned(),
            )),
            Some(instr_val) => {
                *instr_val = new_instr_val;
                self.get_function(context).replace_value(
                    context,
                    old_instr_val,
                    new_instr_val,
                    Some(*self),
                );
                Ok(())
            }
        }
    }

    /// Split the block into two.
    ///
    /// This will create a new block and move the instructions at and following `split_idx` to it.
    /// Returns both blocks.
    pub fn split_at(&self, context: &mut Context, split_idx: usize) -> (Block, Block) {
        let function = context.blocks[self.0].function;
        if split_idx == 0 {
            // We can just create a new empty block and put it before this one.  We know that it
            // will succeed because self is definitely in the function, so we can unwrap().
            let new_block = function.create_block_before(context, self, None).unwrap();
            // Move the block arguments to the new block. We collect because we want to mutate next.
            #[allow(clippy::needless_collect)]
            let args: Vec<_> = self.arg_iter(context).copied().collect();
            for arg in args.into_iter() {
                match &mut context.values[arg.0].value {
                    ValueDatum::Argument(BlockArgument {
                        block,
                        idx: _,
                        ty: _,
                    }) => {
                        // We modify the Value in place to be a BlockArgument for the new block.
                        *block = new_block;
                    }
                    _ => unreachable!("Block arg value inconsistent"),
                }
                new_block.add_arg(context, arg);
            }
            context.blocks[self.0].args.clear();
            (new_block, *self)
        } else {
            // Again, we know that it will succeed because self is definitely in the function, and
            // so we can unwrap().
            let new_block = function.create_block_after(context, self, None).unwrap();

            // Split the instructions at the index and append them to the new block.
            let mut tail_instructions = context.blocks[self.0].instructions.split_off(split_idx);
            context.blocks[new_block.0]
                .instructions
                .append(&mut tail_instructions);

            // If the terminator of the old block (now the new block) was a branch then we need to
            // update the destination block's preds.
            //
            // Copying the candidate blocks and putting them in a vector to avoid borrowing context
            // as immutable and then mutable in the loop body.
            for to_block in match new_block.get_terminator(context) {
                Some(Instruction::Branch(to_block)) => {
                    vec![to_block.block]
                }
                Some(Instruction::ConditionalBranch {
                    true_block,
                    false_block,
                    ..
                }) => {
                    vec![true_block.block, false_block.block]
                }

                _ => Vec::new(),
            } {
                to_block.replace_pred(context, self, &new_block);
            }

            (*self, new_block)
        }
    }

    /// Return an instruction iterator for each instruction in this block.
    pub fn instruction_iter(&self, context: &Context) -> InstructionIterator {
        InstructionIterator::new(context, self)
    }
}

/// An iterator over each block in a [`Function`].
pub struct BlockIterator {
    blocks: Vec<generational_arena::Index>,
    next: usize,
}

impl BlockIterator {
    /// Return a new iterator for each block in `function`.
    pub fn new(context: &Context, function: &Function) -> Self {
        // Copy all the current block indices, so they may be modified in the context during
        // iteration.
        BlockIterator {
            blocks: context.functions[function.0]
                .blocks
                .iter()
                .map(|block| block.0)
                .collect(),
            next: 0,
        }
    }
}

impl Iterator for BlockIterator {
    type Item = Block;

    fn next(&mut self) -> Option<Block> {
        if self.next < self.blocks.len() {
            let idx = self.next;
            self.next += 1;
            Some(Block(self.blocks[idx]))
        } else {
            None
        }
    }
}
