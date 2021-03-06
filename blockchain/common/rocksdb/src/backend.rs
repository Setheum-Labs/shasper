// Copyright 2019 Parity Technologies (UK) Ltd.
// This file is part of Parity Shasper.

// Parity Shasper is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option) any
// later version.

// Parity Shasper is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE.  See the GNU General Public License for more
// details.

// You should have received a copy of the GNU General Public License along with
// Parity Shasper.  If not, see <http://www.gnu.org/licenses/>.
use core::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, RwLock};
use blockchain::{Block, Auxiliary};
use blockchain::backend::{Store, ChainQuery, SharedCommittable, ChainSettlement, Operation};
use parity_codec::{Encode, Decode};
use rocksdb::{DB, Options};

use super::{RocksState, Error};
use super::settlement::RocksSettlement;
use super::utils::*;

pub struct RocksBackend<B: Block, A: Auxiliary<B>, S> {
	db: Arc<DB>,
	head: Arc<RwLock<B::Identifier>>,
	genesis: Arc<B::Identifier>,
	_marker: PhantomData<(B, A, S)>,
}

impl<B: Block, A: Auxiliary<B>, S> RocksBackend<B, A, S> where
	B::Identifier: Decode
{
	fn options() -> Options {
		let mut db_opts = Options::default();
		db_opts.create_missing_column_families(true);
		db_opts.create_if_missing(true);

		db_opts
	}
}

impl<B: Block, A: Auxiliary<B>, S> Clone for RocksBackend<B, A, S> {
	fn clone(&self) -> Self {
		Self {
			db: self.db.clone(),
			head: self.head.clone(),
			genesis: self.genesis.clone(),
			_marker: PhantomData,
		}
	}
}

impl<B: Block, A: Auxiliary<B>, S> Store for RocksBackend<B, A, S> {
	type Block = B;
	type Auxiliary = A;
	type State = S;
	type Error = Error;
}

impl<B: Block, A: Auxiliary<B>, S: RocksState> ChainQuery for RocksBackend<B, A, S> where
	B::Identifier: Encode + Decode,
	B: Encode + Decode,
	A: Encode + Decode,
	A::Key: Encode + Decode,
{
	fn head(&self) -> B::Identifier {
		self.head.read().expect("Lock is poisoned").clone()
	}

	fn genesis(&self) -> B::Identifier {
		self.genesis.as_ref().clone()
	}

	fn contains(
		&self,
		id: &B::Identifier
	) -> Result<bool, Error> {
		Ok(fetch_block_data::<B, S::Raw>(&self.db, id)?.is_some())
	}

	fn is_canon(
		&self,
		id: &B::Identifier
	) -> Result<bool, Error> {
		Ok(fetch_block_data::<B, S::Raw>(&self.db, id)?.ok_or(Error::NotExist)?.is_canon)
	}

	fn lookup_canon_depth(
		&self,
		depth: usize,
	) -> Result<Option<B::Identifier>, Error> {
		let depth = depth as u64;

		let cf = self.db.cf_handle(COLUMN_CANON_DEPTH_MAPPINGS).ok_or(Error::Corrupted)?;
		match self.db.get_cf(cf, depth.encode())? {
			Some(hash) => Ok(Some(B::Identifier::decode(&mut hash.as_ref()).ok_or(Error::Corrupted)?)),
			None => Ok(None),
		}
	}

	fn auxiliary(
		&self,
		key: &A::Key
	) -> Result<Option<A>, Error> {
		let cf = self.db.cf_handle(COLUMN_AUXILIARIES).ok_or(Error::Corrupted)?;
		match self.db.get_cf(cf, key.encode())? {
			Some(v) => Ok(Some(A::decode(&mut v.as_ref()).ok_or(Error::Corrupted)?)),
			None => Ok(None),
		}
	}

	fn children_at(
		&self,
		id: &B::Identifier,
	) -> Result<Vec<B::Identifier>, Error> {
		Ok(fetch_block_data::<B, S::Raw>(&self.db, id)?.ok_or(Error::NotExist)?.children)
	}

	fn depth_at(
		&self,
		id: &B::Identifier
	) -> Result<usize, Error> {
		Ok(fetch_block_data::<B, S::Raw>(&self.db, id)?.ok_or(Error::NotExist)?.depth as usize)
	}

	fn block_at(
		&self,
		id: &B::Identifier,
	) -> Result<B, Error> {
		Ok(fetch_block_data::<B, S::Raw>(&self.db, id)?.ok_or(Error::NotExist)?.block)
	}

	fn state_at(
		&self,
		id: &B::Identifier,
	) -> Result<Self::State, Error> {
		Ok(S::from_raw(
			fetch_block_data::<B, S::Raw>(&self.db, id)?.ok_or(Error::NotExist)?.state,
			self.db.clone()
		))
	}
}

impl<B: Block, A: Auxiliary<B>, S: RocksState> SharedCommittable for RocksBackend<B, A, S> where
	B::Identifier: Encode + Decode,
	B: Encode + Decode,
	A: Encode + Decode,
	A::Key: Encode + Decode,
{
	type Operation = Operation<Self::Block, Self::State, Self::Auxiliary>;

	fn commit(
		&self,
		operation: Operation<Self::Block, Self::State, Self::Auxiliary>,
	) -> Result<(), Self::Error> {
		let mut settlement = RocksSettlement::new(self);
		operation.settle(&mut settlement)?;

		let mut head = self.head.write().expect("Lock is poisoned");
		let new_head = settlement.commit()?;

		if let Some(new_head) = new_head {
			*head = new_head;
		}

		Ok(())
	}
}

impl<B: Block, A: Auxiliary<B>, S: RocksState> RocksBackend<B, A, S> where
	B::Identifier: Encode + Decode,
	B: Encode + Decode,
	A: Encode + Decode,
	A::Key: Encode + Decode,
{
	pub fn open_or_create<P: AsRef<Path>, F>(path: P, f: F) -> Result<Self, Error> where
		F: FnOnce(Arc<DB>) -> Result<(B, S), Error>
	{
		let db_opts = Self::options();
		let db = Arc::new(DB::open_cf(&db_opts, path, &[
			COLUMN_BLOCKS, COLUMN_CANON_DEPTH_MAPPINGS, COLUMN_AUXILIARIES, COLUMN_INFO,
		])?);

		let head = fetch_head(&db)?;
		let genesis = fetch_genesis(&db)?;

		match (head, genesis) {
			(Some(head), Some(genesis)) => {
				Ok(Self {
					db: db,
					head: Arc::new(RwLock::new(head)),
					genesis: Arc::new(genesis),
					_marker: PhantomData,
				})
			},
			(None, None) => {
				let (block, state) = f(db.clone())?;
				assert!(block.parent_id().is_none(),
						"with_genesis must be provided with a genesis block");

				let head = block.id();
				let genesis = head.clone();

				let backend = Self {
					db: db,
					head: Arc::new(RwLock::new(head.clone())),
					genesis: Arc::new(genesis.clone()),
					_marker: PhantomData,
				};

				let mut settlement = RocksSettlement::new(&backend);
				settlement.insert_block(
					genesis.clone(),
					block,
					state,
					0,
					Vec::new(),
					true
				);
				settlement.insert_canon_depth_mapping(0, genesis.clone());
				settlement.set_genesis(genesis.clone());
				settlement.set_head(genesis.clone());
				settlement.commit()?;

				Ok(backend)
			},
			_ => Err(Error::Corrupted),
		}
	}

	pub fn new_with_genesis<P: AsRef<Path>>(path: P, block: B, state: S) -> Result<Self, Error> {
		let mut created = false;
		let backend = Self::open_or_create(path, |_| {
			created = true;
			Ok((block, state))
		})?;
		if !created {
			return Err(Error::Corrupted);
		}
		Ok(backend)
	}

	pub fn from_existing<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
		Self::open_or_create(path, |_| Err(Error::Corrupted))
	}

	pub(crate) fn db(&self) -> &DB {
		self.db.as_ref()
	}
}
