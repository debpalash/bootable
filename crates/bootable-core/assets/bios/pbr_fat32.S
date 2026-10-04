# Bootable FAT32 volume boot record that loads \BOOTMGR (legacy BIOS).
#
# Original work, licensed Apache-2.0. No Microsoft/ms-sys/Syslinux/GRUB code.
# Bytes 0..2 (jump) and 90..509 (code) come from this file; bytes 3..89 are the
# BPB, which the Rust installer copies from the filesystem tool's own boot
# sector (and patches: hidden sectors, drive number, geometry).
#
# Contract implemented (the same one GRUB's and Syslinux's NT loaders use to
# start bootmgr; it is described in docs/legacy-bios.md): BOOTMGR is read from
# the root directory into 2000:0000, then entered at 2000:0000 with DL = BIOS
# drive, DS = ES = SS = 0, SP = 7C00 and the boot sector (BPB) intact at 0000:7C00
# with BS_DrvNum (0x40) = DL and BPB_HiddSec = partition start LBA.
#
# Limits (validated by the installer before this code is written):
# 512-byte sectors, <= 64 sectors/cluster, BOOTMGR <= 0x7F000 bytes.
	.code16
	.text
	.globl _start
	# Variables live just below the boot sector (bp = 0x7c00) so every access
	# is a short disp8 form; the stack starts at 0x7b00 and grows away from them.
	.equ	V_FAT,  -4		# first FAT sector (absolute LBA)
	.equ	V_DATA, -8		# LBA of cluster 2
	.equ	V_CLUS, -12		# current cluster
	.equ	V_LBA,  -16		# directory sector being scanned
	.equ	V_LEFT, -18		# sectors left in the directory cluster
_start:
	.byte	0xeb, 0x58, 0x90	# jmp short start (offset 0x5a)
	.skip	87, 0			# BPB, filled in by the installer
start:
	cli
	xorw	%ax, %ax
	movw	%ax, %ss
	movw	$0x7b00, %sp
	movw	%ax, %ds
	movw	%ax, %es
	sti
	cld
	movw	$0x7c00, %bp
	movb	%dl, 0x40(%bp)		# BS_DrvNum = boot drive
	movzwl	0x0e(%bp), %eax
	addl	0x1c(%bp), %eax
	movl	%eax, V_FAT(%bp)
	movzbl	0x10(%bp), %ecx
	movl	0x24(%bp), %eax
	mull	%ecx
	addl	V_FAT(%bp), %eax
	movl	%eax, V_DATA(%bp)
	movl	0x2c(%bp), %eax
	movl	%eax, V_CLUS(%bp)

dircluster:
	movl	V_CLUS(%bp), %eax
	call	clus2lba
	movl	%eax, V_LBA(%bp)
	movzbw	0x0d(%bp), %ax
	movw	%ax, V_LEFT(%bp)
dirsector:
	movl	V_LBA(%bp), %eax
	movw	$0x7e00, %bx
	movw	$1, %cx
	call	read
	movw	$0x7e00, %si
	movw	$16, %dx
scan:
	cmpb	$0, (%si)
	je	missing
	testb	$0x18, 0x0b(%si)	# skip directories, labels, LFN entries
	jnz	skip
	movw	$11, %cx
	movw	$bootmgr_name, %di
	pushw	%si
	repe cmpsb
	popw	%si
	je	found
skip:
	addw	$32, %si
	decw	%dx
	jnz	scan
	incl	V_LBA(%bp)
	decw	V_LEFT(%bp)
	jnz	dirsector
	movl	V_CLUS(%bp), %eax
	call	nextclus
	cmpl	$0x0ffffff8, %eax
	jae	missing
	movl	%eax, V_CLUS(%bp)
	jmp	dircluster

found:
	cmpl	$0x7f000, 0x1c(%si)	# keep the image below the BIOS data area
	ja	missing
	movzwl	0x14(%si), %eax
	shll	$16, %eax
	movw	0x1a(%si), %ax
	pushw	$0x2000
	popw	%es
load:
	movl	%eax, V_CLUS(%bp)
	call	clus2lba
	movzbw	0x0d(%bp), %cx
	xorw	%bx, %bx
	call	read
	movzbw	0x0d(%bp), %ax
	shlw	$5, %ax			# 512 bytes * sectors / 16 = paragraphs
	movw	%es, %cx
	addw	%ax, %cx
	movw	%cx, %es
	cmpw	$0x9f00, %cx		# corrupt/cyclic chain guard
	jae	missing
	movl	V_CLUS(%bp), %eax
	call	nextclus
	cmpl	$0x0ffffff8, %eax
	jb	load
	xorw	%ax, %ax
	movw	%ax, %es
	movb	0x40(%bp), %dl
	movw	%bp, %sp
	ljmp	$0x2000, $0

# eax = cluster -> eax = first sector (clobbers ecx, edx)
clus2lba:
	subl	$2, %eax
	movzbl	0x0d(%bp), %ecx
	mull	%ecx
	addl	V_DATA(%bp), %eax
	ret

# eax = cluster -> eax = next cluster (FAT entry & 0x0fffffff)
nextclus:
	pushw	%es
	pushw	%bx
	pushl	%edx
	movl	%eax, %edx
	shrl	$7, %eax
	addl	V_FAT(%bp), %eax
	pushw	%cx
	pushw	$0
	popw	%es
	movw	$0x8000, %bx
	movw	$1, %cx
	call	read
	popw	%cx
	andw	$127, %dx
	shlw	$2, %dx
	movw	%dx, %bx
	movl	0x8000(%bx), %eax
	andl	$0x0fffffff, %eax
	popl	%edx
	popw	%bx
	popw	%es
	ret

# eax = LBA, cx = sector count, es:bx = destination. Preserves registers.
read:
	pushal
	pushl	$0
	pushl	%eax
	pushw	%es
	pushw	%bx
	pushw	%cx
	pushw	$16
	movw	%sp, %si
	movb	0x40(%bp), %dl
	movb	$0x42, %ah
	int	$0x13
	jc	diskerr
	addw	$16, %sp
	popal
	ret

diskerr:
missing:
	movw	$m_err, %si
fail:
	xorw	%ax, %ax
	movw	%ax, %ds
	movw	%ax, %es
1:	lodsb
	testb	%al, %al
	jz	2f
	movb	$0x0e, %ah
	movw	$7, %bx
	int	$0x10
	jmp	1b
2:	hlt
	jmp	2b

bootmgr_name:	.ascii	"BOOTMGR    "
m_err:		.asciz	"BOOTMGR error\r\n"

	.org	510
	.word	0xaa55
