# Bootable legacy-BIOS MBR bootstrap (440 bytes).
#
# Original work, licensed Apache-2.0 like the rest of Bootable. It contains no
# Microsoft, Rufus, ms-sys, Syslinux, or GRUB code. It implements the generic
# MBR contract: relocate, find the one partition flagged active (0x80), read
# its first sector with INT 13h extensions, verify 0x55AA, and jump to it with
# DL = BIOS drive and DS:SI = the partition-table entry.
#
# Build (see build.sh):  as --32 mbr.S && ld -m elf_i386 -Ttext 0x7c00 --oformat binary
	.code16
	.text
	.globl _start
_start:
	cli
	xorw	%ax, %ax
	movw	%ax, %ss
	movw	$0x7c00, %sp
	movw	%ax, %ds
	movw	%ax, %es
	sti
	cld
	movw	$0x7c00, %si
	movw	$0x0600, %di
	movw	$256, %cx
	rep movsw
	ljmp	$0, $(relocated - _start + 0x0600)

relocated:
	movw	$(0x0600 + 0x1be), %si
	movw	$4, %cx
1:	cmpb	$0x80, (%si)
	je	2f
	addw	$16, %si
	loop	1b
	movw	$(m_none - _start + 0x0600), %si
	jmp	fail
2:	movw	%si, %bp		# bp = active partition entry
	pushw	%dx			# keep the BIOS drive number
	pushl	$0			# DAP: LBA high
	pushl	8(%si)			# DAP: LBA low (partition start)
	pushw	$0			# DAP: segment
	pushw	$0x7c00			# DAP: offset
	pushw	$1			# DAP: one sector
	pushw	$16			# DAP: size
	movw	%sp, %si
	movb	$0x42, %ah
	int	$0x13
	jc	3f
	addw	$16, %sp
	popw	%dx
	cmpw	$0xaa55, 0x7dfe
	jne	4f
	movw	%bp, %si
	ljmp	$0, $0x7c00
3:	movw	$(m_read - _start + 0x0600), %si
	jmp	fail
4:	movw	$(m_sig - _start + 0x0600), %si
fail:
	lodsb
	testb	%al, %al
	jz	hang
	movb	$0x0e, %ah
	movw	$7, %bx
	int	$0x10
	jmp	fail
hang:
	hlt
	jmp	hang

m_none:	.asciz	"No active partition\r\n"
m_read:	.asciz	"Disk read error\r\n"
m_sig:	.asciz	"Invalid boot sector\r\n"

	.org	440
