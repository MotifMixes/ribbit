; Execution test programs for ribbit-cpu (tests/exec.rs).
; Built by scripts/gen-cpu-vectors.py:
;   arc-elf32-as -mcpu=arc600 -mmul64 -mnorm -mswap -mEA
;   arc-elf32-ld -Ttext=0x40000000   (vector table at INT_VECTOR_BASE)
; Each snippet ends in brk_s; the test harness starts at the snippet's label
; with sp = 0x3c100000 and checks registers against PRM semantics.
	.text
	.global _vectors
_vectors:
	j	_bad		; 0 reset (tests set PC directly)
	j	_bad		; 1 memory error
	j	vec2_handler	; 2 instruction error / SWI
	j	vec3_handler	; 3 timer 0
	.rept 12
	j	_bad		; 4..15
	.endr
	j	vec16_handler	; 16
	.rept 15
	j	_bad		; 17..31
	.endr
_bad:
	brk_s

; ---- arithmetic and flags --------------------------------------------------
	.global t_add_overflow
t_add_overflow:
	mov	r1,0x7fffffff
	add.f	r2,r1,1
	brk_s

	.global t_add_carry
t_add_carry:
	mov	r1,-1
	add.f	r2,r1,1
	brk_s

	.global t_adc
t_adc:
	mov	r1,-1
	add.f	r0,r1,1
	mov	r3,5
	adc	r4,r3,10
	brk_s

	.global t_sub_borrow
t_sub_borrow:
	mov	r1,3
	sub.f	r2,r1,5
	brk_s

	.global t_sbc
t_sbc:
	mov	r1,3
	sub.f	r0,r1,5
	mov	r3,10
	sbc.f	r4,r3,4
	brk_s

	.global t_sub_overflow
t_sub_overflow:
	mov	r1,0x80000000
	sub.f	r2,r1,1
	brk_s

	.global t_cmp_conds
t_cmp_conds:
	mov	r1,-1
	mov	r2,1
	cmp	r1,r2
	mov	r10,0
	mov.lt	r10,1
	mov	r11,0
	mov.hi	r11,1
	mov	r12,0
	mov.c	r12,1
	mov	r13,0
	mov.ge	r13,1
	mov	r14,0
	mov.ne	r14,1
	brk_s

	.global t_scaled_add
t_scaled_add:
	mov	r1,100
	mov	r2,3
	add1	r3,r1,r2
	add2	r4,r1,r2
	add3	r5,r1,r2
	sub1	r6,r1,r2
	sub2	r7,r1,r2
	sub3	r8,r1,r2
	rsub	r9,r2,r1
	brk_s

; ---- logic and bits --------------------------------------------------------
	.global t_logic
t_logic:
	mov	r1,0xf0f0
	and	r2,r1,0xff
	or	r3,r1,0x0f
	bic	r4,r1,0xf0
	xor	r5,r1,0xffff
	bset	r6,r1,0
	bclr	r7,r1,4
	bxor	r8,r1,15
	bmsk	r9,r1,7
	bmsk	r10,r1,31
	not	r11,r1
	brk_s

	.global t_btst_tst
t_btst_tst:
	mov	r1,0x80
	btst	r1,7
	mov	r2,0
	mov.eq	r2,1
	btst	r1,6
	mov	r3,0
	mov.eq	r3,1
	tst	r1,0x80
	mov	r4,0
	mov.ne	r4,1
	brk_s

; ---- shifts and rotates ----------------------------------------------------
	.global t_asl_single
t_asl_single:
	mov	r1,0x80000001
	asl.f	r2,r1
	brk_s

	.global t_shift_single
t_shift_single:
	mov	r1,0x80000001
	asr	r2,r1
	lsr	r3,r1
	ror	r4,r1
	brk_s

	.global t_rrc_rlc
t_rrc_rlc:
	mov	r1,-1
	add.f	r0,r1,1
	mov	r1,2
	rrc.f	r2,r1
	rlc	r3,r1
	brk_s

	.global t_multi_shift
t_multi_shift:
	mov	r1,0x81
	asl.f	r2,r1,4
	mov	r3,0x80000000
	asr	r4,r3,4
	lsr.f	r5,r1,1
	ror	r6,r1,4
	asl	r7,r1,63
	brk_s

	.global t_extend
t_extend:
	mov	r1,0x1234ff80
	sexb	r2,r1
	sexw	r3,r1
	extb	r4,r1
	extw	r5,r1
	mov	r6,-5
	abs	r7,r6
	neg	r8,r6
	mov	r12,r6
	neg_s	r13,r12
	brk_s

	.global t_maxmin
t_maxmin:
	mov	r1,-3
	mov	r2,5
	max.f	r3,r1,r2
	min	r4,r1,r2
	brk_s

; ---- multiply --------------------------------------------------------------
	.global t_mul64
t_mul64:
	mov	r1,0x12345678
	mov	r2,0x9abcdef0
	mul64	r1,r2
	mov	r10,r57
	mov	r11,r58
	mov	r12,r59
	mulu64	r1,r2
	mov	r13,r57
	mov	r14,r58
	mov	r15,r59
	brk_s

	.global t_mulhi
t_mulhi:
	sr	0xdeadbeef,[mulhi]
	mov	r1,r59
	brk_s

; ---- memory ----------------------------------------------------------------
	.global t_loadstore
t_loadstore:
	mov	r1,0x3c000100
	mov	r2,0x11223344
	st	r2,[r1]
	ldb	r3,[r1,0]
	ldb	r4,[r1,3]
	ldw	r5,[r1,2]
	mov	r6,0x8080
	stw	r6,[r1,4]
	ldw.x	r7,[r1,4]
	ldb.x	r8,[r1,4]
	st.ab	r2,[r1,8]
	st.a	r2,[r1,4]
	ld.as	r9,[r1,-3]
	mov	r10,r1
	brk_s

	.global t_pushpop
t_pushpop:
	mov	blink,0x1234
	mov	r1,7
	push_s	r1
	push_s	blink
	mov	r1,0
	mov	blink,0
	pop_s	blink
	pop_s	r1
	mov	r3,sp
	brk_s

	.global t_gp
t_gp:
	mov	gp,0x3c000200
	mov	r1,0x5555aaaa
	st	r1,[gp,8]
	ld_s	r0,[gp,8]
	brk_s

	.align	4
	.global t_pcl
t_pcl:
	ld_s	r1,[pcl,8]
	brk_s
	nop_s
	nop_s
	.long	0xcafef00d

; ---- control flow ----------------------------------------------------------
	.global t_bdelay
t_bdelay:
	mov	r1,0
	mov	r2,0
	b.d	1f
	add	r1,r1,1
	add	r2,r2,1
1:	b	2f
	add	r1,r1,10
2:	mov	r4,1
	cmp	r4,0
	beq.d	3f
	add	r1,r1,100
	add	r1,r1,1000
3:	brk_s

	.global t_bl
t_bl:
	bl	sub1
	.global ret1
ret1:
	mov	r5,r0
	bl.d	sub1
	mov	r6,1
	.global ret2
ret2:
	mov	r7,r0
	brk_s
	.align	4
sub1:
	mov	r0,blink
	j_s	[blink]

	.global t_brcc
t_brcc:
	mov	r1,5
	mov	r2,0
	brlt	r1,6,1f
	add	r2,r2,1
1:	brhs	r1,6,2f
	add	r2,r2,10
2:	mov	r3,-1
	brlo	r3,r1,3f
	add	r2,r2,100
3:	brlt	r3,r1,4f
	add	r2,r2,1000
4:	bbit1	r1,2,5f
	add	r2,r2,10000
5:	bbit0	r1,0,6f
	add	r2,r2,100000
6:	mov_s	r0,0
	breq_s	r0,0,7f
	add	r2,r2,1000000
7:	brk_s

	.global t_jumps
t_jumps:
	mov	r2,0
	mov	r1,jt1
	j.d	[r1]
	add	r2,r2,1
	add	r2,r2,10
jt1:
	mov	r3,jt2
	jl	[r3]
	.global jl_ret
jl_ret:
	brk_s
jt2:
	mov	r4,blink
	j_s	[blink]

	.global t_loop
t_loop:
	mov	r1,0
	mov	lp_count,5
	lp	1f
	add	r1,r1,1
	add	r1,r1,10
1:	brk_s

	.global t_loop_skipped
t_loop_skipped:
	mov	r1,0
	mov	r2,1
	cmp	r2,1
	lpne	1f
	add	r1,r1,1
1:	brk_s

; ---- auxiliary registers, flags, interrupts ----------------------------------
	.global t_aux
t_aux:
	lr	r1,[identity]
	mov	r2,0x12345
	sr	r2,[int_vector_base]
	lr	r3,[int_vector_base]
	mov	r5,0
	cmp	r5,0
	lr	r4,[status32]
	sr	0x4001,[lp_start]
	lr	r6,[lp_start]
	lr	r7,[0x3f0]
	brk_s

	.global t_flag
t_flag:
	flag	0x806
	lr	r1,[status32]
	brk_s

	.global t_flag_halt
t_flag_halt:
	flag	1
	nop
	nop
	nop

	.global t_irq
t_irq:
	flag	2
	sleep
	.global irq_resume
irq_resume:
	mov	r9,1
	brk_s
	.global vec16_handler
vec16_handler:
	mov	r20,ilink1
	brk_s
	j.f	[ilink1]

	.global t_swi
t_swi:
	swi
	.global swi_ret
swi_ret:
	brk_s
	.global vec2_handler
vec2_handler:
	mov	r21,ilink2
	j.f	[ilink2]

	.global t_timer
t_timer:
	sr	0,[control0]
	sr	10,[limit0]
	sr	1,[control0]
	sr	0,[count0]
	flag	2
	sleep
	.global timer_resume
timer_resume:
	brk_s
	.global vec3_handler
vec3_handler:
	mov	r23,ilink1
	sr	0,[control0]
	j.f	[ilink1]

; ---- options: norm, swap, extended arithmetic --------------------------------
	.global t_options
t_options:
	mov	r1,0x1fffffff
	norm	r2,r1
	mov	r1,0
	norm	r3,r1
	mov	r1,0x12345678
	swap	r4,r1
	mov	r1,0x7fffffff
	adds.f	r5,r1,1
	mov	r1,0x80000000
	subs	r6,r1,1
	mov	r1,0x7fff0001
	mov	r8,0x00010001
	addsdw	r7,r1,r8
	mov	r1,0x12345
	sat16	r9,r1
	mov	r1,0x80000000
	abss	r10,r1
	negs	r11,r1
	mov	r1,0x40000000
	asls	r12,r1,1
	mov	r1,-16
	asrs	r13,r1,2
	mov	r15,-2
	asls	r14,r1,r15
	brk_s

; ---- dual 16x16 multiply-accumulate (MULDW/MACDW/MSUBDW) ---------------------
	.global t_dsp
t_dsp:
	mov	r1,8
	sr	r1,[aux_macmode]
	mov	r1,0x40004000
	mov	r2,0x20002000
	muldw	r3,r1,r2
	macdw	r4,r1,r2
	msubdw	r5,r1,r2
	mov	r6,0x7fff7fff
	muldw	r7,r6,r6
	macdw	r8,r6,r6
	lr	r9,[aux_macmode]
	mov	r1,0
	sr	r1,[aux_macmode]
	mov	r1,0x7fff8000
	mov	r2,0x7fff7fff
	muldw	r10,r1,r2
	mov	r1,0x12348000
	rnd16	r11,r1
	mov	r1,0x7fffc000
	rnd16	r12,r1
	brk_s
