; Encoding corpus for the ARCtangent-A5 decoder. Assembled with
;   arc-elf32-as -mcpu=arc600 -mmul64 -mnorm -mswap -mEA
; and disassembled with arc-elf32-objdump -d; see scripts/gen-cpu-vectors.py.
; Written for coverage of encodings, not to be executed.
	.text
	.global _start
_start:
; ---- 32-bit ALU, all operand formats -------------------------------------
	add	r1,r2,r3
	add.f	r1,r2,r3
	add	r1,r2,63
	add	r1,r1,-2048
	add	r1,r1,2047
	add.eq	r1,r1,r2
	add.ne.f r1,r1,5
	add	r1,0x12345678,r3
	add	r1,r2,0x87654321
	add	0,r2,r3
	adc	r4,r5,r6
	sub	r4,r5,r6
	sbc.f	r4,r5,r6
	and	r7,r8,r9
	or	r7,r8,0x3f
	bic	r7,r8,r9
	xor.f	r10,r11,r12
	max	r1,r2,r3
	min.f	r1,r2,r3
	mov	r1,r2
	mov	r1,0x30
	mov	r1,-1
	mov.gt	r1,r2
	mov.f	r1,0xdeadbeef
	tst	r1,r2
	cmp	r1,r2
	cmp	r1,0x12345678
	rcmp	r1,r2
	rsub	r1,r2,r3
	bset	r1,r2,31
	bclr	r1,r2,r3
	btst	r1,7
	bxor	r1,r2,3
	bmsk	r1,r2,15
	add1	r1,r2,r3
	add2	r1,r2,r3
	add3	r1,r2,r3
	sub1	r1,r2,r3
	sub2	r1,r2,r3
	sub3	r1,r2,r3
	add.hi	r1,r1,r2
	add.ls	r1,r1,r2
	add.pnz	r1,r1,r2
	add.vs	r1,r1,r2
	add.vc	r1,r1,r2
	add.ge	r1,r1,r2
	add.lt	r1,r1,r2
	add.le	r1,r1,r2
	add.mi	r1,r1,r2
	add.pl	r1,r1,r2
	add.cs	r1,r1,r2
	add.cc	r1,r1,r2
; ---- single operand ---------------------------------------------------------
	asl	r1,r2
	asr.f	r1,r2
	lsr	r1,r2
	ror	r1,r2
	rrc	r1,r2
	sexb	r1,r2
	sexw	r1,r2
	extb	r1,r2
	extw	r1,r2
	abs	r1,r2
	not	r1,r2
	rlc	r1,r2
	asl	r1,0x3f
	not	r1,0x11223344
; ---- zero operand / control -------------------------------------------------
	sleep	0
	sleep	5
	swi
; brk (32-bit): PRM basecase, but binutils rejects it for arc600
	flag	r1
	flag	3
	flag.eq	r2
	lr	r1,[r2]
	lr	r1,[0x12]
	lr	r1,[0x1234]
	sr	r1,[r2]
	sr	r1,[0x25]
	sr	0x12345678,[0x43]
	nop
; ---- barrel shifter and options (major 0x05) --------------------------------
	asl	r1,r2,r3
	lsr	r1,r2,5
	asr	r1,r2,r3
	ror	r1,r2,r3
	asl.f	r1,r1,7
	mul64	r1,r2
	mulu64	r1,0x1234
	mul64	r1,0x12345678
	adds	r1,r2,r3
	subs	r1,r2,r3
	divaw	r1,r2,r3
	asls	r1,r2,r3
	asrs	r1,r2,r3
	addsdw	r1,r2,r3
	subsdw	r1,r2,r3
	swap	r1,r2
	norm	r1,r2
	sat16	r1,r2
	rnd16	r1,r2
	abssw	r1,r2
	abss	r1,r2
	negsw	r1,r2
	negs	r1,r2
	normw	r1,r2
; ---- loads and stores -------------------------------------------------------
	ld	r1,[r2]
	ld	r1,[r2,4]
	ld	r1,[r2,-256]
	ld	r1,[r2,255]
	ldb	r1,[r2,1]
	ldb.x	r1,[r2,1]
	ldw	r1,[r2,2]
	ldw.x	r1,[r2,2]
	ld.a	r1,[r2,4]
	ld.ab	r1,[r2,4]
	ld.as	r1,[r2,4]
	ldw.as	r1,[r2,4]
	ld.di	r1,[r2,8]
	ld	r1,[0x12345678]
	ld	r1,[r2,r3]
	ldb.x	r1,[r2,r3]
	ldw.ab	r1,[r2,r3]
	ld	r1,[r2,0x12345678]
	ld	r1,[0x12345678,r3]
	st	r1,[r2]
	st	r1,[r2,-4]
	stb	r1,[r2,1]
	stw	r1,[r2,2]
	st.a	r1,[r2,-4]
	st.ab	r1,[r2,4]
	st.as	r1,[r2,3]
	st.di	r1,[r2,0]
	st	r1,[0x12345678]
	st	0x11223344,[r2,8]
; ---- branches ----------------------------------------------------------------
1:	b	1b
	b.d	2f
	nop
2:	beq	1b
	bne.d	2b
	nop
	bgt	2b
	b	_far
	bl	1b
	bl.d	2b
	nop
	blne	1b
	bl	_far
	breq	r1,r2,1b
	brne.d	r1,r2,2b
	nop
	brlt	r1,7,2b
	brge	r1,r2,2b
	brlo	r1,r2,2b
	brhs	r1,63,2b
	breq	r1,0x12345678,2b
	bbit0	r1,r2,2b
	bbit1.d	r1,31,2b
	nop
	j	[r1]
	j.d	[r2]
	nop
	j	0x12345678
	j	0x20
	jeq	[r3]
	jl	[r1]
	jl.d	[r2]
	nop
	jl	0x12345678
	jlne	[blink]
	j.f	[ilink1]
	j.f	[ilink2]
	lp	3f
	nop
	nop
3:	lpne	4f
	nop
4:
; ---- 16-bit -----------------------------------------------------------------
	ld_s	r0,[r1,r2]
	ldb_s	r3,[r12,r13]
	ldw_s	r14,[r15,r0]
	add_s	r1,r2,r3
	add_s	r1,r2,7
	sub_s	r1,r2,3
	asl_s	r1,r2,1
	asr_s	r1,r2,7
	add_s	r0,r0,r20
	mov_s	r0,r20
	cmp_s	r0,r20
	mov_s	r20,r0
	add_s	r0,r0,0x12345678
	mov_s	r1,0x12345678
	cmp_s	r1,0x12345678
	j_s	[r1]
	j_s.d	[r2]
	nop_s
	jl_s	[r3]
	jl_s.d	[r12]
	nop_s
	sub_s.ne r1,r1,r1
	nop_s
; unimp_s: PRM basecase, but binutils rejects it for arc600
	jeq_s	[blink]
	jne_s	[blink]
	j_s	[blink]
	j_s.d	[blink]
	nop_s
	sub_s	r1,r1,r2
	and_s	r1,r1,r2
	or_s	r1,r1,r2
	bic_s	r1,r1,r2
	xor_s	r1,r1,r2
	tst_s	r1,r2
	mul64_s	r1,r2
	sexb_s	r1,r2
	sexw_s	r1,r2
	extb_s	r1,r2
	extw_s	r1,r2
	abs_s	r1,r2
	not_s	r1,r2
	neg_s	r1,r2
	add1_s	r1,r1,r2
	add2_s	r1,r1,r2
	add3_s	r1,r1,r2
	asl_s	r1,r1,r2
	lsr_s	r1,r1,r2
	asr_s	r1,r1,r2
	asl_s	r1,r2
	asr_s	r1,r2
	lsr_s	r1,r2
	brk_s
	ld_s	r1,[r2,124]
	ldb_s	r1,[r2,31]
	ldw_s	r1,[r2,62]
	ldw_s.x	r1,[r2,2]
	st_s	r1,[r2,4]
	stb_s	r1,[r2,5]
	stw_s	r1,[r2,6]
	asl_s	r1,r1,31
	lsr_s	r1,r1,1
	asr_s	r1,r1,2
	sub_s	r1,r1,3
	bset_s	r1,r1,4
	bclr_s	r1,r1,5
	bmsk_s	r1,r1,6
	btst_s	r1,7
	ld_s	r1,[sp,124]
	ldb_s	r1,[sp,8]
	st_s	r1,[sp,4]
	stb_s	r1,[sp,12]
	add_s	r1,sp,64
	add_s	sp,sp,16
	sub_s	sp,sp,16
	pop_s	r1
	pop_s	blink
	push_s	r13
	push_s	blink
	ld_s	r0,[gp,-1024]
	ld_s	r0,[gp,1020]
	ldb_s	r0,[gp,-256]
	ldw_s	r0,[gp,510]
	add_s	r0,gp,-4
	ld_s	r1,[pcl,1020]
	mov_s	r1,255
	add_s	r1,r1,127
	cmp_s	r1,127
5:	breq_s	r1,0,5b
	brne_s	r1,0,5b
	b_s	5b
	beq_s	5b
	bne_s	5b
	bgt_s	5b
	bge_s	5b
	blt_s	5b
	ble_s	5b
	bhi_s	5b
	bhs_s	5b
	blo_s	5b
	bls_s	5b
	bl_s	_far
	.align 4
_far:
	nop
