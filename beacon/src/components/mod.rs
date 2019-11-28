type Epoch = u64;
type Balance = u64;

pub trait Checkpoint: Clone {
	fn epoch(&self) -> Epoch;
}

pub trait Registry {
	type Checkpoint;
	type Error;

	fn total_active_balance(&self) -> Balance;
	fn attesting_target_balance(
		&self,
		checkpoint: &Self::Checkpoint
	) -> Result<Balance, Self::Error>;
}

pub struct JustificationProcessor<C: Checkpoint> {
	justification_bits: [bool; 4],
	current_justified_checkpoint: C,
	previous_justified_checkpoint: C,
	finalized_checkpoint: C,
}

impl<C: Checkpoint> JustificationProcessor<C> {
	pub fn advance_epoch<R: Registry<Checkpoint=C>>(
		&mut self,
		previous_checkpoint: C,
		current_checkpoint: C,
		registry: &R
	) -> Result<(), R::Error> {
		let current_epoch = current_checkpoint.epoch();
		let old_previous_justified_checkpoint = self.previous_justified_checkpoint.clone();
		let old_current_justified_checkpoint = self.current_justified_checkpoint.clone();

		// Process justifications
		self.previous_justified_checkpoint = self.current_justified_checkpoint.clone();
		let old_justification_bits = self.justification_bits.clone();
		let justification_bits_len = self.justification_bits.len();
		self.justification_bits[1..].copy_from_slice(
			&old_justification_bits[0..(justification_bits_len - 1)]
		);
		self.justification_bits[0] = false;

		if registry.attesting_target_balance(&previous_checkpoint)? * 3 >=
			registry.total_active_balance() * 2
		{
			self.current_justified_checkpoint = previous_checkpoint;
			self.justification_bits[1] = true;
		}
		if registry.attesting_target_balance(&current_checkpoint)? * 3 >=
			registry.total_active_balance() * 2
		{
			self.current_justified_checkpoint = current_checkpoint;
			self.justification_bits[0] = true;
		}

		// Process finalizations
		let bits = self.justification_bits.clone();
		// The 2nd/3rd/4th most recent epochs are justified,
		// the 2nd using the 4th as source
		if bits[1..4].iter().all(|v| *v) &&
			old_previous_justified_checkpoint.epoch() + 3 == current_epoch
		{
			self.finalized_checkpoint = old_previous_justified_checkpoint.clone();
		}
		// The 2nd/3rd most recent epochs are justified,
		// the 2nd using the 3rd as source
		if bits[1..3].iter().all(|v| *v) &&
			old_previous_justified_checkpoint.epoch() + 2 == current_epoch
		{
			self.finalized_checkpoint = old_previous_justified_checkpoint.clone();
		}
		// The 1st/2nd/3rd most recent epochs are justified,
		// the 1st using the 3rd as source
		if bits[0..3].iter().all(|v| *v) &&
			old_current_justified_checkpoint.epoch() + 2 == current_epoch
		{
			self.finalized_checkpoint = old_current_justified_checkpoint.clone();
		}
		// The 1st/2nd most recent epochs are justified,
		// the 1st using the 2nd as source
		if bits[0..2].iter().all(|v| *v) &&
			old_current_justified_checkpoint.epoch() + 1 == current_epoch
		{
			self.finalized_checkpoint = old_current_justified_checkpoint.clone();
		}

		Ok(())
	}
}
