# Test double for Windows' BIOS bootmgr: NOT Microsoft code, NOT a Windows
# loader. It only reports, on COM1, what the boot chain handed it, so
# scripts/qemu-usb-bios-smoke.sh can prove the MBR and FAT32 boot record
# located \BOOTMGR, loaded every cluster, and entered it with the documented
# register/BPB contract. Original work, Apache-2.0.
	.code16
	.text
	.globl _start
	.equ	SIZE, 0x14000		# larger than 64 KiB, spans several FAT sectors
	.equ	TAILOFF, SIZE - 0x10000 - 4	# offset of "TAIL" within segment 0x3000
_start:
	cli
	movb	%dl, %bl		# DL at entry
	pushw	%cs
	popw	%ds
	movw	$msg_head, %si
	call	puts
	movb	%bl, %al
	call	hex8
	movw	$msg_drv, %si
	call	puts
	movb	%es:0x7c40, %al		# BPB BS_DrvNum
	call	hex8
	movw	$msg_hid, %si
	call	puts
	movb	%es:0x7c1f, %al		# BPB_HiddSec, big-endian digits
	call	hex8
	movb	%es:0x7c1e, %al
	call	hex8
	movb	%es:0x7c1d, %al
	call	hex8
	movb	%es:0x7c1c, %al
	call	hex8
	movw	$msg_seg, %si
	call	puts
	movw	%cs, %ax
	movb	%ah, %al
	call	hex8
	movw	%cs, %ax
	call	hex8
	movw	$msg_tail, %si
	call	puts
	movw	$0x3000, %ax
	movw	%ax, %fs
	cmpl	$0x4c494154, %fs:TAILOFF	# "TAIL"
	jne	bad
	movw	$msg_ok, %si
	jmp	done
bad:	movw	$msg_bad, %si
done:	call	puts
1:	hlt
	jmp	1b

puts:	lodsb
	testb	%al, %al
	jz	9f
	call	putc
	jmp	puts
9:	ret

hex8:	pushw	%ax
	shrb	$4, %al
	call	nibble
	popw	%ax
	andb	$0x0f, %al
nibble:	addb	$'0', %al
	cmpb	$'9', %al
	jbe	putc
	addb	$7, %al
putc:	pushw	%dx
	pushw	%ax
	movw	$0x3fd, %dx
1:	inb	%dx, %al
	testb	$0x20, %al
	jz	1b
	popw	%ax
	movw	$0x3f8, %dx
	outb	%al, %dx
	popw	%dx
	ret

msg_head:	.asciz	"BOOTMGR-STUB dl="
msg_drv:	.asciz	" drv="
msg_hid:	.asciz	" hid="
msg_seg:	.asciz	" cs="
msg_tail:	.asciz	" tail="
msg_ok:		.asciz	"OK\r\n"
msg_bad:	.asciz	"BAD\r\n"

	.org	SIZE - 4
	.ascii	"TAIL"
