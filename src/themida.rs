use binaryninja::{
    architecture::Architecture,
    basicblock::BasicBlock,
    binaryview::{BinaryView, BinaryViewExt},
    command::Command,
    function::Function,
    llil,
};
use log::debug;

use crate::{search_for_code_entries, CodeEntryDescription, CodeEntryDestRange};

pub struct ThemidaSpotterCommand;

impl Command for ThemidaSpotterCommand {
    fn action(&self, view: &BinaryView) {
        let target_sections = view
            .sections()
            .iter()
            .filter(|section| {
                let name = section.name();
                // Note: Themida/WinLicense 3.x only
                name.as_str() == ".boot"
                    || name.as_str() == ".themida"
                    || name.as_str() == ".winlice"
                    || name.as_str() == ".vlizer"
            })
            .map(|section| CodeEntryDestRange {
                start: section.start(),
                end: section.end(),
            })
            .collect();

        search_for_code_entries(view, search_for_themida_code_entries, target_sections)
    }

    fn valid(&self, _view: &BinaryView) -> bool {
        true
    }
}

fn search_for_themida_code_entries(
    bv: &BinaryView,
    func: &Function,
    themida_section_ranges: &[CodeEntryDestRange],
) -> Option<CodeEntryDescription> {
    debug!("Processing '{}'", func.symbol().full_name());

    // Check if we're in the correct section (i.e., out of Themida's sections)
    let func_addr = func.start();
    if themida_section_ranges
        .iter()
        .any(|r| r.contains(&func_addr))
    {
        return None;
    }

    let llil_func = func.low_level_il().ok()?;
    // TODO(ergrelet): search any basic block, not just the first one, as
    // functions might be only partially obfuscated
    if let Some(first_block) = llil_func.basic_blocks().iter().next() {
        if let Some(first_inst) = first_block.iter().next() {
            // Match `jmp imm` instruction
            if let llil::InstrInfo::TailCall(op) = first_inst.info() {
                if let llil::ExprInfo::ConstPtr(const_operation) = op.target().info() {
                    let jmp_destination = const_operation.value();
                    // Check if jmp destination is inside of Themida's section
                    if themida_section_ranges
                        .iter()
                        .any(|r| r.contains(&jmp_destination))
                    {
                        // We're in an obfuscated code entry
                        // We now need to figure out whether it's mutated or virtualized
                        if destination_is_vmenter(bv, jmp_destination) {
                            debug!(
                                "Themida VMEnter detected at 0x{:x} ('{}')",
                                op.address(),
                                func.symbol().full_name(),
                            );
                            return Some(CodeEntryDescription::VMEnter(op.address()));
                        }

                        // Doesn't look virtualized, assume it's mutated
                        debug!(
                            "Themida MUTEnter detected at 0x{:x} ('{}')",
                            op.address(),
                            func.symbol().full_name(),
                        );
                        return Some(CodeEntryDescription::MUTEnter(first_inst.address()));
                    }
                }
            }
        }
    }

    None
}

/// Return `true` if the given destination VA looks like a VMEnter routine.
/// Return `false` otherwise.
fn destination_is_vmenter(bv: &BinaryView, destination_addr: u64) -> bool {
    // Iterate over all potential functions
    for code_entry_func in bv.functions_at(destination_addr).into_iter() {
        if let Ok(llil_code_entry_func) = code_entry_func.low_level_il() {
            // Check if function looks like a VMEnter routine
            if function_is_vm_enter(llil_code_entry_func.as_ref()) {
                return true;
            }
        }
    }

    false
}

/// Return `true` if the given LLIL function looks like Themida's VMEnter routine.
///
/// This checks if the first instruction is `pushfd` and that the function exits
/// with a `jmp [reg]` instruction.
fn function_is_vm_enter<A: Architecture>(
    function: &llil::Function<A, llil::Finalized, llil::NonSSA<llil::RegularNonSSA>>,
) -> bool {
    if let Some(first_block) = function.basic_blocks().iter().next() {
        // Check if first block looks like the start of a VMEnter and one basic
        // block looks like the end of a VMEnter
        if block_is_vmenter_start(first_block.as_ref())
            && function
                .basic_blocks()
                .iter()
                .any(|block| block_is_vmenter_end(block.as_ref()))
        {
            return true;
        }
    }

    false
}

/// Return `true` if the given basic block looks like the first basic block of
/// a VMEnter routine (i.e., starts with a `pushfd` instruction).
/// Return `false` otherwise.
fn block_is_vmenter_start<A: Architecture>(
    block: &BasicBlock<llil::LowLevelBlock<A, llil::Finalized, llil::NonSSA<llil::RegularNonSSA>>>,
) -> bool {
    if let Some(first_inst) = block.iter().next() {
        // Match a `pushfd` instruction (VMEnter)
        if instruction_is_pushfd(&first_inst) {
            return true;
        }
    }

    false
}

/// Return `true` if the given LLIL instruction corresponds to a `pushfd` instruction.
/// Return `false` otherwise.
fn instruction_is_pushfd<A: Architecture>(
    instruction: &llil::Instruction<'_, A, llil::Finalized, llil::NonSSA<llil::RegularNonSSA>>,
) -> bool {
    // LLIL instruction should be a push
    if let llil::InstrInfo::Push(op) = instruction.info() {
        // Operand should be a `or` (with many flags)
        if let llil::ExprInfo::Or(_) = op.operand().info() {
            return true;
        }
    }

    false
}

/// Return `true` if the given basic block looks like the final basic block of
/// a VMEnter routine (i.e., ends with a `jmp [reg]` instruction).
/// Return `false` otherwise.
fn block_is_vmenter_end<A: Architecture>(
    block: &BasicBlock<llil::LowLevelBlock<A, llil::Finalized, llil::NonSSA<llil::RegularNonSSA>>>,
) -> bool {
    // Check if last instruction is `jmp [rax/eax]`
    if let Some(last_ins) = block.iter().last() {
        if let llil::InstrInfo::Jump(jmp_operation) = last_ins.info() {
            if let llil::ExprInfo::Load(load_operation) = jmp_operation.target().info() {
                if let llil::ExprInfo::Reg(_) = load_operation.source_mem_expr().info() {
                    return true;
                }
            }
        }
    }

    false
}
